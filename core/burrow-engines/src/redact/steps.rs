//! The redaction steps, against a real document.
//!
//! `redact::Steps` is the seam the assembly drives; everything in #131 was tested against a
//! fake implementation of it. This is the real one, and the resolver's experience is why it
//! exists before verification does: meeting real documents produced three findings no test on
//! the milestone had caught, and this touches more engine surface than the resolver did.
//!
//! # It owns the document, and that is the contract
//!
//! `Steps`' rustdoc requires it: every step consumes the redaction, so the poisoned-document
//! rule holds only if dropping the `Steps` value drops the document. An implementation holding
//! `&mut Document` would leave the caller able to write out a half-edited one. This takes its
//! `PdfDocument` by value (#191: either engine's), and the only path to bytes is
//! `redact::Finished::emit_verified`, which takes the read-back.
//!
//! # Crate-internal, and reached only through a verified path
//!
//! Nothing here is exported. The operation is `burrow_ops::redact::page`, which goes through
//! [`crate::PageRedactor`] — whose only route to a `Vec<u8>` is `Finished::emit_verified`, and
//! that takes the read-back as a parameter. ADR 0022's rule is the signature rather than a
//! comment now.

use core::ffi::c_int;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Result};

use super::frame::Reading;
use super::resources::PageResources;
use super::sharing::{FormUseCounts, count_form_uses};
use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::geometry::{
    FormsReached, Glyph, InkKind, NamedProperties, PropertyList, Rect, ScopedFont, Watch,
    carried_text_edits, check_form_sharing, check_type_three_image_inside,
    check_type_three_procedure, glyphs_and_ink_in, glyphs_in, remove_glyphs_and_carried_text,
};
use crate::pdfsyntax::region::{PageFrame, Region};
use crate::pdfsyntax::tounicode::ToUnicode;
use crate::redact::graph::{PdfDocument, PdfObject};
use crate::redact::{FontOutcome, Steps, StreamId};

const CONTENTS: Name = Name::literal(b"/Contents\0");
const FONT: Name = Name::literal(b"/Font\0");
const WIDTHS: Name = Name::literal(b"/Widths\0");
const TO_UNICODE: Name = Name::literal(b"/ToUnicode\0");
const ENCODING: Name = Name::literal(b"/Encoding\0");
const DIFFERENCES: Name = Name::literal(b"/Differences\0");

/// The longest `/Differences` array this will rewrite.
///
/// Two passes of `array_len()` FFI calls each, and `array_len` is the attacker's number. The
/// value is [`crate::pdfsyntax::ops::MAX_COMPOSITE_ITEMS`]'s, because a `/Differences` is an
/// array and that is already the cap this codebase puts on how long an array may be.
const DIFFERENCES_CEILING: c_int = 65_536;

/// One page's redaction, against a live document of either engine.
pub(crate) struct PageRedaction<D> {
    document: D,
    /// Which page is being redacted.
    ///
    /// **Which pages the whole operation covers is not held here.** The assembly passes that
    /// set to `cut_fonts`, and a second copy on this struct is a second thing that can be
    /// wrong: the font-cutting rule turns on it, and two sources of one set is how they stop
    /// agreeing.
    page: usize,
    region: Region,
    /// The ceilings this operation applies. Carried because `page_contents` decodes content
    /// streams and has to stop somewhere; see its header for the measurement.
    limits: burrow_types::Limits,
    /// Counted once, before any edit: the sharing rule is about the document as it arrived.
    sharing: FormUseCounts,
    deadline: Deadline,
    clock: Arc<dyn Clock>,
    /// The glyphs the region reaches, resolved once by `affected_streams`.
    cut: Vec<Glyph>,
    /// The `Do` names in the page's own stream that lead to a removed glyph.
    ///
    /// Resolved once in `affected_streams` and used again in `rewrite`, because recomputing it
    /// there would be a second answer to "which spans cover a removal" — and this seam's whole
    /// history is two answers that differed by exactly the leak.
    page_draws: BTreeSet<Vec<u8>>,
    /// The same, per form on a path to a removed glyph.
    form_draws: BTreeMap<u64, BTreeSet<Vec<u8>>>,
    /// What the page stream's `BDC` names resolve to, through the page's own `/Properties`.
    ///
    /// Resolved once, with the scope, for the reason `page_draws` is: `rewrite` must decide on
    /// the same answer `affected_streams` did. **No step adds a text-carrying key to any
    /// dictionary** -- every edit after this removes keys or rewrites stream data -- so a list
    /// read here as carrying nothing cannot carry text in the emitted document.
    page_properties: NamedProperties,
    /// The same, per form, through the scope each form's names resolve against.
    form_properties: BTreeMap<u64, NamedProperties>,
    /// How many property lists have had their carried text dropped, for §7's disclosure.
    dropped_carried_text: usize,
    /// What the reference pass read (#227), kept for the walk that asks what still names an
    /// annotation the region removed (#239). `None` until that pass has run.
    referenced: Option<super::references::Referenced>,
}

impl<D: PdfDocument> PageRedaction<D> {
    /// The operation's deadline, as the geometry walk reads it.
    ///
    /// THE ONLY PLACE ONE IS BUILT, so every walk this type runs spends the same deadline on the
    /// same clock. A code review replaced the deadline at each of the seven call sites in turn
    /// with a fresh one, and six of the seven swaps failed no test; one constructor is one site
    /// to get right and one site a test can reach.
    fn watch(&self) -> Watch<'_> {
        Watch::new(self.deadline, self.clock.as_ref())
    }

    /// Begin a redaction of `page` over `region`.
    ///
    /// # Errors
    ///
    /// Whatever opening or walking the document failed with, including every refusal the walk
    /// and the sharing rule raise.
    pub(crate) fn new(
        document: D,
        page: usize,
        region: Region,
        limits: burrow_types::Limits,
        deadline: Deadline,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        // THE BOUND, established here rather than in one caller, and with the error that names
        // the rule. `page_handle` once relied on it in a SAFETY comment -- "`self.page` was
        // checked against the page count when the redaction was built" -- while nothing in this
        // constructor checked it: the check lived in `redact_page_for_probe`, and `new` is
        // `pub(crate)`, so a second crate-internal caller got undefined behaviour under a comment
        // saying it could not happen. Since #191 the lookup is bounds-checked behind
        // `PdfDocument::page` as well, so this is the check that names the rule, not the one
        // that makes the call safe.
        let count = usize::try_from(document.page_count()?)
            .map_err(|_| Error::Internal("a page count that does not fit in usize".to_owned()))?;
        if page >= count {
            return Err(Error::InvalidArgument(
                "pdf redaction [page-out-of-range]: a page index past the end of the document"
                    .to_owned(),
            ));
        }
        // COUNTED BEFORE ANY EDIT. The sharing rule is about the document as it arrived; a
        // count taken after a rewrite would be counting the operation's own work.
        let sharing = count_form_uses(&document, &deadline, &clock)?;
        Ok(Self {
            document,
            page,
            region,
            limits,
            sharing,
            deadline,
            clock,
            cut: Vec::new(),
            page_draws: BTreeSet::new(),
            form_draws: BTreeMap::new(),
            page_properties: NamedProperties::default(),
            form_properties: BTreeMap::new(),
            dropped_carried_text: 0,
            referenced: None,
        })
    }

    /// Every page this operation covers, as indices that exist in the document.
    ///
    /// **The page being edited is always in it**, even when the caller's set does not name it:
    /// its codes are the ones the content edit just changed, and a font cut without them is a
    /// font cut against a page nobody looked at. An index past the end is a refusal rather
    /// than a skip — the native page lookup is unsafe on one, and a silently skipped page is a
    /// page whose codes are treated as no longer drawn.
    fn pages_in_scope(&self, redacted: &BTreeSet<usize>) -> Result<Vec<usize>> {
        let count = usize::try_from(self.document.page_count()?)
            .map_err(|_| Error::Internal("a page count that does not fit in usize".to_owned()))?;
        let mut pages: BTreeSet<usize> = redacted.clone();
        pages.insert(self.page);
        for at in &pages {
            if *at >= count {
                return Err(Error::InvalidArgument(
                    "pdf redaction [page-out-of-range]: the operation covers a page the \
                     document does not have"
                        .to_owned(),
                ));
            }
        }
        Ok(pages.into_iter().collect())
    }

    fn page_handle(&self) -> Result<D::Object<'_>> {
        // `self.page` was checked against the page count when the redaction was built, with the
        // error that names the rule. `page` checks again, which is what makes the lookup safe
        // rather than a comment, and drains as this did.
        self.document.page(self.page)
    }

    /// Refuse when anything the removal does not take still names what it takes (#239).
    ///
    /// **What the removal takes** is each annotation removed and each Popup one names (see
    /// `owned_by`). **What names it** is read from every object the file references, each
    /// unparsed as qpdf holds it now, and from the trailer qpdf holds and will write; so a page, an
    /// action on the
    /// catalogue, a resource dictionary, an annotation on another page, or the trailer's `/Info`.
    ///
    /// **Any such name is a dependent kept for an independent reason** (owner, 2026-10-01), and is
    /// refused: kept, it writes the removed annotation out -- measured, its `/Contents` and its
    /// appearance in the output after an `Ok`. With nothing naming it, qpdf does not write it, nor
    /// anything only it reached, and nothing is edited to make that so. The first version of #239
    /// emptied each removed annotation in place instead, and both reviews showed why that was
    /// wrong: it emptied whatever else the object was -- a page's graphics state, another page's
    /// font, the trailer's `/Info` -- without a word; and it left anything the annotation reached
    /// that something else also named.
    ///
    /// A kept annotation that loses a `/Popup` entry (see `remove_annotations_in`) is held to the
    /// same rule, less this page's own listing of it: named by another page, it would change there
    /// too, and is refused.
    ///
    /// **And the page's `/Annots` array, where the pass erased from it and it is its own object**
    /// (#240): erasing changes it wherever else it is named, so anything but this page's own
    /// dictionary naming it refuses. A per-page copy was measured not to help -- its entries are
    /// the same objects on every page that names it.
    ///
    /// An object nothing references is neither read nor written, so an orphan naming a removed
    /// annotation refuses nothing. The walk runs only when something with an identity was removed,
    /// and reads the deadline at every object.
    fn refuse_what_the_removal_leaves(
        &self,
        removed: &Removed,
        page: &D::Object<'_>,
    ) -> Result<()> {
        const ANNOTS: Name = Name::literal(b"/Annots\0");
        if removed.annotations.is_empty()
            && removed.popups.is_empty()
            && removed.edited.is_empty()
            && removed.annots_erased.is_none()
        {
            return Ok(());
        }
        // DEFENCE IN DEPTH, unwitnessed: `run` always calls the reference pass before the steps,
        // so this cannot be `None`; refusing is the answer if a later order ever makes it so.
        let Some(referenced) = &self.referenced else {
            return Err(Error::Internal(
                "pdf redaction: the reference pass did not run before the annotation walk"
                    .to_owned(),
            ));
        };
        let mut seeds = removed.annotations.clone();
        seeds.extend(removed.popups.iter().copied());
        let owned = self.owned_by(&seeds)?;
        // THIS PAGE'S OWN LISTING of an annotation it keeps and edits.
        let mut listing = BTreeSet::new();
        let this_page = identity_of(page)?;
        listing.extend(this_page);
        // THIS PAGE'S `/Annots` COUNTS AS ITS OWN LISTING only because #240 refuses below when
        // anything else names that array: a shared array exempted here would let an edited kept
        // annotation change on the other page too. Keep the two together.
        listing.extend(identity_of(&page.key(&ANNOTS))?);
        page.drained()?;
        let refusal = || {
            Error::Unsupported(
                "pdf redaction [annotation-dependent-kept]: something the redaction keeps still \
                 names an annotation the region removes, or what it owns, which would write it out \
                 again"
                    .to_owned(),
            )
        };
        if referenced
            .from_trailer
            .iter()
            .any(|named| owned.contains(named) || removed.edited.contains(named))
        {
            return Err(refusal());
        }
        // A SHARED `/Annots` ARRAY (#240): the edit erased from an array that is its own object, so
        // it changes wherever else that array is named -- another page, the catalogue, a field, an
        // annotation, the trailer. Refused rather than copied per page: every entry of a shared
        // array is the same object on every page that names it, so a copy would leave the removed
        // annotation on the other page and in the file (measured, owner's decision 2026-10-02).
        let shared = || {
            Error::Unsupported(
                "pdf redaction [annotation-dependent-kept]: the region removes an annotation from \
                 an /Annots array something else also names, so the removal would change it there \
                 too"
                .to_owned(),
            )
        };
        if removed
            .annots_erased
            .is_some_and(|array| referenced.from_trailer.contains(&array))
        {
            return Err(shared());
        }
        for &(number, generation) in &referenced.objects {
            self.deadline.checkpoint(self.clock.as_ref())?;
            if owned.contains(&(number, generation)) {
                continue;
            }
            let object = self.document.object(number, generation)?;
            let text = match object.type_code() {
                object_type::DICTIONARY | object_type::ARRAY => object.unparse(),
                object_type::STREAM => object.stream_dict().unparse(),
                _ => {
                    object.drained()?;
                    continue;
                }
            };
            object.drained()?;
            let mut checkpoint = || self.deadline.checkpoint(self.clock.as_ref());
            let found = crate::pdfsyntax::references::in_value(&text, &mut checkpoint)?;
            // DEFENCE IN DEPTH, unwitnessed: qpdf unparses every reference in plain digits, so
            // nothing it writes reads as irregular, and the mutation that deletes this survives.
            // Kept because an answer the lexer calls incomplete must not be taken as "nothing".
            if found.irregular.is_some() {
                return Err(refusal());
            }
            for named in &found.references {
                if owned.contains(named) {
                    return Err(refusal());
                }
                if removed.annots_erased == Some(*named) && this_page != Some((number, generation))
                {
                    return Err(shared());
                }
                if removed.edited.contains(named) && !listing.contains(&(number, generation)) {
                    return Err(refusal());
                }
            }
        }
        Ok(())
    }

    /// The removed annotations and every Popup one of them names, by identity: what must be named
    /// by nothing the redaction keeps. A Popup is its annotation's own (owner, 2026-10-01), so one
    /// the page does not list goes with it as one it lists does.
    ///
    /// **Nothing else a removed annotation reaches is followed**, and that was measured, not
    /// assumed: a first version followed every key, and refused 49 of 107 regions over the real
    /// documents' own annotations that main redacted -- a link's `/Dest` reaches a page, which the
    /// page tree names. What a removed annotation alone reaches, nothing kept names once the
    /// annotation is gone, so qpdf does not write it; what something kept names as well is kept for
    /// that, and is that thing's.
    fn owned_by(&self, removed: &BTreeSet<(c_int, c_int)>) -> Result<BTreeSet<(c_int, c_int)>> {
        let mut owned = removed.clone();
        let mut pending: Vec<(c_int, c_int)> = removed.iter().copied().collect();
        while let Some((number, generation)) = pending.pop() {
            self.deadline.checkpoint(self.clock.as_ref())?;
            let object = self.document.object(number, generation)?;
            match object.type_code() {
                object_type::DICTIONARY => match popup_of(&object)? {
                    Popup::At(popup) => {
                        if owned.insert(popup) {
                            pending.push(popup);
                        }
                    }
                    Popup::Unreadable => return Err(popup_unreadable()),
                    Popup::None => {}
                },
                object_type::NULL => {}
                // A POPUP THAT IS ITS OWN OBJECT BUT NOT A DICTIONARY -- an indirect array, a
                // stream -- names what the walk cannot follow, as one written inline does (#239's
                // fifth specification review: `/Popup 8 0 R` over `8 0 obj [7 0 R]` wrote 7 out
                // after an `Ok`).
                _ => return Err(popup_unreadable()),
            }
            object.drained()?;
        }
        Ok(owned)
    }

    /// Refuse the document if any reference it writes resolves to null, or cannot be read as qpdf
    /// reads it; see `references::refuse_references_to_nothing` (#227). `bytes` is the input the
    /// document was opened from.
    ///
    /// # Errors
    ///
    /// `[reference-to-nothing]`, `[reference-unreadable]`, the deadline, and whatever the engine
    /// reports.
    pub(crate) fn refuse_references_to_nothing(&mut self, bytes: &[u8]) -> Result<()> {
        self.referenced = Some(super::references::refuse_references_to_nothing(
            &self.document,
            bytes,
            &self.deadline,
            self.clock.as_ref(),
        )?);
        Ok(())
    }

    /// Refuse the document if it carries an interactive form — `/AcroForm` on the catalogue, or a
    /// form field (`/FT`) on any referenced object. ADR 0029 §3 refuses a document with an
    /// `/AcroForm`: the carrier sits on the catalogue, out of the page-scoped reach redaction has,
    /// and a field's value `/V` lives in a field object off the page, so a page-side proxy (a
    /// `/Widget` on this page's `/Annots`) misses a field whose widget is on another page, a field
    /// with no widget, and a widget whose `/FT` is inherited from a `/Parent`.
    ///
    /// Two signals, because neither alone is faithful to §3:
    ///
    /// - The catalogue's `/AcroForm`, resolved **through the trailer's `/Root`** rather than by
    ///   scanning referenced objects. `/Root` may be a *direct* dictionary — qpdf accepts one, and
    ///   then the catalogue has no object number and is absent from the reference set, so a scan of
    ///   referenced objects would never see its `/AcroForm` (a security review found exactly this
    ///   leak: the field `/V` survived a region redaction). `PdfObject::key` resolves a direct or an
    ///   indirect `/Root` alike, so reading `/AcroForm` off the resolved catalogue catches the form
    ///   **however its fields are laid out**, including a field written *inline* inside
    ///   `/AcroForm /Fields` (whose `/FT` is a key of the array element, invisible to a `/FT` scan —
    ///   `key` reads one top-level key, it does not descend). `/AcroForm` is valid only on the
    ///   catalogue (ISO 32000-1 §12.7.2, Table 28), so this is the whole of that signal.
    /// - A top-level `/FT` on any referenced object — a field object carrying `/FT` with no
    ///   catalogue `/AcroForm` (`/FT` is a field-dictionary entry, §12.7.3.1, Table 220).
    ///
    /// An annotation that is not a field, under a catalogue with no `/AcroForm` (the near-miss
    /// twin), carries neither and is not refused.
    ///
    /// It is a **deliberate over-refusal**: it refuses every document with a form, not only those
    /// whose field reaches the redacted region (#125, owner 2026-10-07). The narrower "fields
    /// reaching the region" refusal is filed as a post-launch issue (#274).
    ///
    /// # Errors
    ///
    /// `[acroform-field]`, the deadline, and whatever reading an object raises. An `Internal`
    /// if the reference pass did not run first, which `run` always arranges.
    pub(crate) fn refuse_form_fields(&self) -> Result<()> {
        const FIELD_TYPE: Name = Name::literal(b"/FT\0");
        const ACRO_FORM: Name = Name::literal(b"/AcroForm\0");
        const ROOT: Name = Name::literal(b"/Root\0");
        let refusal = || {
            Error::Unsupported(
                "pdf redaction [acroform-field]: the document carries an interactive form \
                 (/AcroForm on the catalogue, or an /FT field), whose field values the /AcroForm \
                 names from the catalogue -- out of the page-scoped reach redaction has, so the \
                 whole document is refused"
                    .to_owned(),
            )
        };
        // THE CATALOGUE'S /AcroForm, resolved through the trailer's /Root. `key` follows a direct
        // or an indirect /Root, so a direct catalogue -- which has no object number and so is in no
        // reference set -- is still read.
        let trailer = self.document.trailer()?;
        let catalog = trailer.key(&ROOT);
        let has_acro_form = catalog.key(&ACRO_FORM).type_code() != object_type::NULL;
        catalog.drained()?;
        trailer.drained()?;
        if has_acro_form {
            return Err(refusal());
        }
        // A FIELD OBJECT carrying /FT, for a form with no catalogue /AcroForm. The catalogue signal
        // above cannot see a bare field object, so the two together are the faithful refusal.
        // DEFENCE IN DEPTH, unwitnessed: `run` always calls the reference pass before this, so
        // this cannot be `None`; refusing is the answer if a later order ever makes it so.
        let Some(referenced) = &self.referenced else {
            return Err(Error::Internal(
                "pdf redaction: the reference pass did not run before the form-field scan"
                    .to_owned(),
            ));
        };
        for &(number, generation) in &referenced.objects {
            self.deadline.checkpoint(self.clock.as_ref())?;
            let object = self.document.object(number, generation)?;
            // A field is a dictionary; `/FT` on a stream would be irregular, but a stream dict is
            // read the same way and fails closed if one ever carries it.
            let is_field = match object.type_code() {
                object_type::DICTIONARY => object.key(&FIELD_TYPE).type_code() != object_type::NULL,
                object_type::STREAM => {
                    object.stream_dict().key(&FIELD_TYPE).type_code() != object_type::NULL
                }
                _ => false,
            };
            // Surface any engine error the reads left on the document before acting on them.
            object.drained()?;
            if is_field {
                return Err(refusal());
            }
        }
        Ok(())
    }

    /// Refuse the page if its `/MediaBox` comes from the page tree and `renderer_size` does not
    /// vouch for it; see `frame::check_inherited_media_box` (#224).
    ///
    /// # Errors
    ///
    /// `[media-box-unverified]`, and whatever reading the page raises.
    pub(crate) fn check_inherited_media_box(
        &self,
        renderer_size: impl FnOnce() -> Result<Option<(f64, f64)>>,
    ) -> Result<()> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        super::frame::check_inherited_media_box(&self.page_handle()?, renderer_size)
    }

    /// The page's frame, for converting the region into content space.
    fn frame<O: PdfObject>(page: &O) -> Result<PageFrame> {
        super::frame::of(page)
    }

    /// The page's content as one lexical stream, with the element handles behind it.
    ///
    /// # Read here rather than taken from qpdf's concatenation
    ///
    /// `qpdf_oh_get_page_content_data` returns the elements already joined and says nothing
    /// about where the joins were. That is enough to *read* a page and not enough to *write*
    /// one back: a rewriter holding only its output can emit the page as a single stream, which
    /// collapses the array and rewrites the object graph of a document that asked for some text
    /// to be removed. Spike 0006 measured the naive route doing exactly that.
    ///
    /// The handles come back in element order beside the map, because the write is per element
    /// and matching by position is the only correspondence there is — two elements can be the
    /// same object, so identity would not distinguish them.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] naming `contents-not-a-stream` for a `/Contents` that is neither a
    /// stream nor an array of streams, and `contents-unreadable` for an element whose data will
    /// not decode. A page with **no** `/Contents` is `Ok(None)` rather than an error: it is a
    /// blank page, which is legal and ordinary.
    fn page_contents<O: PdfObject>(
        &self,
        page: &O,
    ) -> Result<Option<(crate::pdfsyntax::contents::Contents, Vec<O>)>> {
        let contents = page.key(&CONTENTS);
        page.drained()?;
        let handles: Vec<O> = match contents.type_code() {
            object_type::STREAM => vec![contents],
            object_type::ARRAY => {
                let length = contents.array_len();
                contents.drained()?;
                let mut out = Vec::new();
                for at in 0..length {
                    let element = contents.array_item(at);
                    contents.drained()?;
                    if element.type_code() != object_type::STREAM {
                        // AN ELEMENT THAT IS NOT A STREAM IS NOT A GAP TO SKIP. Skipping it
                        // would shift every later element's index, so the map from an offset
                        // back to an object would name the wrong one -- a cut written to the
                        // wrong stream, reported as success.
                        return Err(Error::Malformed(
                            "pdf redaction [contents-not-a-stream]: a /Contents array holding \
                             something that is not a content stream"
                                .to_owned(),
                        ));
                    }
                    out.push(element);
                }
                out
            }
            // ABSENT. `/Contents` is optional (PDF 32000-1 §7.7.3.3) and a page without one
            // is blank -- ordinary, not malformed. Refusing it was an undisclosed regression:
            // the previous commit redacted such a page to a no-op, and this file's own
            // `sharing_tests` argues in a comment that refusing here would refuse a document
            // for a page nobody asked about. Found by a review measuring both commits.
            object_type::NULL => return Ok(None),
            _ => {
                return Err(Error::Malformed(
                    "pdf redaction [contents-not-a-stream]: a page whose /Contents is neither \
                     a stream nor an array of them"
                        .to_owned(),
                ));
            }
        };

        // NO ELEMENT CEILING HERE, and its absence is measured rather than assumed.
        //
        // A review suggested adding one for locality: the walk enforces `MAX_ELEMENTS` in
        // `PageRedaction::new`, so this function's bound was an ordering property of a
        // different function. Adding it made **both** untestable — each ceiling refuses with
        // the same rule name, so deleting either leaves the other producing the same message
        // and a mutation sweep caught neither. Two defences that mask each other are worth one
        // defence and a lost test.
        //
        // So there is one ceiling, in the walk, where it also bounds the walk's own work — and
        // `sharing_tests::a_contents_array_past_the_element_ceiling_is_refused_by_the_walk`
        // asks it directly rather than through an operation that cannot reach it.

        // BOUNDED AND CHECKPOINTED PER ELEMENT, and both were missing.
        //
        // `MAX_ELEMENTS` is 4096 and **the same object may be referenced by every element**, so
        // the decoded total is 4096 x the element's size however small the file is — no
        // compression needed. Measured by a security review, release build: 4096 references to
        // one 1 MB stream is a **1.02 MB input** that reaches **8.2 GB** of resident memory in
        // 2.9 s, and the operation then fails on a ceiling inside `glyphs_in` — after both
        // allocations. On a phone or a browser tab that is an OOM kill rather than a refusal.
        //
        // `max_memory_bytes` detects rather than bounds and its check is after the engine call,
        // so nothing fired first. This is a running total against the same ceiling, tested
        // before each decode, which is the only place it can be tested cheaply: by the time
        // `Contents::concatenate` sees the parts, `bodies` already holds the peak.
        let mut bodies: Vec<Vec<u8>> = Vec::with_capacity(handles.len());
        let mut decoded: u64 = 0;
        for handle in &handles {
            self.deadline.checkpoint(self.clock.as_ref())?;
            let Some(body) = handle.stream_data()? else {
                return Err(Error::Malformed(
                    "pdf redaction [contents-unreadable]: a content stream whose data burrow \
                     could not decode, so what it draws is unknown"
                        .to_owned(),
                ));
            };
            decoded = decoded.saturating_add(u64::try_from(body.len()).unwrap_or(u64::MAX));
            if decoded > self.limits.max_memory_bytes {
                return Err(Error::Unsupported(format!(
                    "pdf redaction [contents-too-large]: a page whose /Contents decodes to \
                     more than the {} bytes this operation will hold",
                    self.limits.max_memory_bytes
                )));
            }
            bodies.push(body);
        }
        let borrowed: Vec<&[u8]> = bodies.iter().map(Vec::as_slice).collect();
        let map = crate::pdfsyntax::contents::Contents::concatenate(&borrowed)?;
        Ok(Some((map, handles)))
    }
}

impl<D: PdfDocument> Steps for PageRedaction<D> {
    fn dropped_carried_text(&self) -> usize {
        self.dropped_carried_text
    }

    fn affected_streams(&mut self) -> Result<Vec<StreamId>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        // OPTIONAL CONTENT FIRST, AND WHETHER OR NOT THE REGION REACHES IT. ADR 0029 §3 refuses
        // optional content referenced by a kept page, and this operation keeps every page; the
        // refusal is about the page, not about the glyphs being cut. Before a blank page's early
        // return too, because an annotation can carry `/OC` on a page that draws nothing.
        // Missing until #166 -- see the module for how a coincidence was standing in for it.
        super::optional_content::refuse_optional_content(
            &page,
            PageResources::of(&page)?.dictionary(),
            &self.deadline,
            &self.clock,
        )?;
        // BURROW'S OWN CONCATENATION, not `qpdf_oh_get_page_content_data`'s.
        //
        // qpdf hands back the elements joined and nothing about where the joins were, so a
        // rewriter holding only its output can write the page back as ONE stream and nothing
        // else -- which silently rewrites the object graph of a document that asked for some
        // text to be removed, and is what spike 0006 measured the naive route doing.
        // `Contents` keeps the boundaries, and every offset below is into its bytes.
        let Some((contents, elements)) = self.page_contents(&page)? else {
            // A blank page draws nothing, so the region reaches nothing and there is no edit
            // to plan. Not a refusal: see `page_contents`.
            return Ok(Vec::new());
        };
        let resources = PageResources::of(&page)?;

        let frame = Self::frame(&page)?;
        let region = self.region.to_content_space(&frame)?;

        // EVERY GLYPH, then the ones the region reaches. The conservative box, not the advance
        // box: a glyph's ink can sit far from its origin, so uncertainty removes more. The same
        // walk also returns the non-text ink -- images, painted paths, shadings -- which the
        // redaction cannot remove (#125).
        let (glyphs, ink) = glyphs_and_ink_in(contents.bytes(), &resources, &self.watch())?;
        // NON-TEXT INK THE REGION REACHES is refused before any edit is planned. Redaction removes
        // glyphs, not ink, so a secret drawn as an image or a path in the region would survive an
        // `Ok` -- ADR 0029 §3 (the image and vector-path rows) and §8's forbidden outcome. The box
        // is fail closed (see `InkBox`): larger than the ink, and the whole page for a bare `sh`.
        if let Some(reached) = ink.iter().find(|box_| box_.reaches(&region)) {
            return Err(Error::Unsupported(format!(
                "pdf redaction [{}]: the redacted region reaches {}, which the redaction removes \
                 no part of, so the region cannot be cleared",
                reached.kind.rule(),
                match reached.kind {
                    InkKind::Image => "an image",
                    InkKind::Vector => "vector path content",
                    InkKind::Shading => "a shading",
                },
            )));
        }
        let cut: Vec<Glyph> = glyphs
            .iter()
            .filter(|glyph| glyph.conservative_box().intersects(&region))
            .cloned()
            .collect();

        // THE SHARING RULE, before any edit is planned. A form the region reaches that is
        // drawn elsewhere is refused rather than edited -- editing it removes its text from a
        // page nobody selected, which §6's read-back cannot see.
        check_form_sharing(&cut, &self.sharing)?;

        // THE SAME RULE FOR THE PAGE'S OWN CONTENT, which the form rule did not cover: a glyph
        // drawn by the page rather than by a form has `form: None`, and nothing counted those.
        // Checked PER ELEMENT rather than per page, because only the element the region
        // actually reaches matters -- a two-element `/Contents` whose letterhead element is
        // shared and whose body element is not is redactable, and refusing the page would
        // refuse a document that could have been served.
        check_contents_sharing(&cut, &contents, &elements, &self.sharing, self.page)?;

        // THE TYPE 3 RULE, the same shape and for the same reason. A glyph procedure is a
        // content stream the walk does not descend into, so text inside one is text nothing
        // observed -- ADR 0029 §8's forbidden outcome. `check_type_three_procedure` refuses a
        // procedure that shows any text; this is the part that finds the procedures to hand it.
        // EVERY FONT THE PAGE DRAWS WITH, not the fonts the region reached. See the
        // function's header for what the narrower scope missed.
        // THE SCOPE TRAVELS WITH THE NAME, because `ScopedFont` will not let it not. This
        // collected bare names and `check_type_three` resolved them against the page, which
        // shadowed a form's Type 3 font behind a page decoy and left the secret in the output.
        let drawn_fonts: BTreeSet<ScopedFont> = glyphs
            .iter()
            .map(|glyph| glyph.source.font.clone())
            .collect();
        let image_fonts = check_type_three(&resources, &drawn_fonts, &self.watch())?;
        // A TYPE 3 GLYPH DRAWN AS AN IMAGE, cut (#125, owner's decision 2026-10-08). Its box
        // covers the image, so the region removes it -- and its procedure, the bitmap, stays in
        // `/CharProcs`. Refused when the region reaches any glyph of a font that draws one.
        // Keyed on the font, not the code: resolving a code to its procedure is the name
        // resolution `check_type_three`'s header says was wrong twice.
        if cut
            .iter()
            .any(|glyph| image_fonts.contains(&glyph.source.font))
        {
            return Err(Error::Unsupported(
                "pdf redaction [type-three-image-cut]: the region reaches a Type 3 glyph drawn as \
                 an image, whose bitmap would stay in the font after the glyph is removed"
                    .to_owned(),
            ));
        }

        // THE MARKED-CONTENT RULE, per stream, because a `Span` indexes the stream it was read
        // from. The page's own content first, then each form the removal reaches -- a form
        // carries its own `BDC`s and its own offsets.
        //
        // EVERY STREAM ON A PATH TO A REMOVED GLYPH, each told which of its own `Do`s lead to
        // one. A span in one stream can cover glyphs drawn from another, and the honest scope
        // is the whole path rather than its endpoints -- see `marked_content_scope` for the two
        // shapes a narrower scope leaked through, both measured.
        //
        // `FormsReached::Unresolved` is gone from this call site with it: the scope resolves
        // each form's own names now, so there is nothing left to be unresolved about.
        let reached_forms: BTreeSet<u64> =
            cut.iter().filter_map(|glyph| glyph.source.form).collect();
        let scope = marked_content_scope(&resources, &reached_forms, &self.deadline, &self.clock)?;
        // REFUSES ONLY WHAT THE REWRITER CANNOT HANDLE. `/ActualText` and `/Alt` written out
        // are dropped by `carried_text_edits` in `rewrite`. A property list named through
        // `/Properties` is resolved against the scope that drew the stream (#166): an ordinary
        // one passes, one carrying text is refused by name, and one that cannot be read still
        // refuses as unresolved.
        // ONE CALL PER STREAM, NOT TWO. `check_marked_content` is now
        // `carried_text_edits(..).map(|_| ())`
        // -- same walk, same early return, and its only remaining refusal (`Unknown`) is raised
        // by the rewriter too. Asking twice cost a second `find_form` per form below, and that
        // loop is the one whose own comment records 22.5 s against a 100 ms deadline.
        let page_carries = !carried_text_edits(
            contents.bytes(),
            &cut,
            None,
            &FormsReached::Named(&scope.page_names),
            &scope.page_properties,
            &self.watch(),
        )?
        .is_empty();
        let mut carrying_forms: Vec<u64> = Vec::new();
        for (form, names, properties) in &scope.forms {
            // A CHECKPOINT PER FORM, because this loop is the attacker's number. `form_scope` is
            // bounded only by the visit budget (~4095), and each iteration calls `find_form`,
            // which re-walks the resource graph from the page with a **fresh** budget of its own.
            // A security review measured 22.5 s against a `max_duration_ms` of 100 -- 225x, where
            // `CLAUDE.md` promises overshoot "of up to one engine call".
            //
            // The quadratic is not fixed here and is not a regression -- `main`'s lookup had the
            // same shape at 19.1 s. What changed is that the deadline is consulted while the work
            // happens, and that this is **one** loop: a first draft of the rewriter asked
            // `check_marked_content` here and `carried_text_edits` again below, doubling the
            // `find_form` calls this comment is about. `marked_content_scope` already holds each
            // form's handle and could hand it over, removing the re-walk entirely; filed.
            self.deadline.checkpoint(self.clock.as_ref())?;
            if !carried_text_edits(
                &find_form(&resources, *form)?,
                &cut,
                Some(*form),
                &FormsReached::Named(names),
                properties,
                &self.watch(),
            )?
            .is_empty()
            {
                carrying_forms.push(*form);
            }
        }

        let mut streams: Vec<StreamId> = cut
            .iter()
            .map(|glyph| glyph.source.form.map_or(StreamId::Page, StreamId::Object))
            .collect();
        // AND EVERY STREAM CARRYING A SPAN OVER A REMOVAL, which is not the same set. A page
        // whose only involvement is `/Span << /ActualText … >> BDC /X1 Do EMC` has no removed
        // glyph of its own, so it was not in this list -- and the stream holding the text to
        // drop would never have been rewritten. The same cross-stream shape as the refusal this
        // replaces, one step further on.
        if page_carries {
            streams.push(StreamId::Page);
        }
        streams.extend(carrying_forms.iter().copied().map(StreamId::Object));
        streams.sort_unstable();
        streams.dedup();
        drop(resources);
        drop(elements);
        drop(page);
        // ASSIGNED AFTER THE HANDLES ARE DROPPED. `page` borrows `self`, so recording the scope
        // before that point borrows it twice.
        self.page_draws = scope.page_names;
        self.page_properties = scope.page_properties;
        for (form, names, properties) in scope.forms {
            self.form_draws.insert(form, names);
            self.form_properties.insert(form, properties);
        }
        self.cut = cut;
        Ok(streams)
    }

    fn rewrite(&mut self, stream: StreamId) -> Result<()> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        let resources = PageResources::of(&page)?;
        let null = page.null_beside();

        // THE MATCH YIELDS THE COUNT rather than assigning into a local declared above it: every
        // arm sets it, so an initial value would be dead and the compiler says so.
        let dropped_here = match stream {
            // THE PAGE'S OWN CONTENT, WRITTEN BACK ELEMENT BY ELEMENT. An array `/Contents` is
            // one lexical stream to read and several objects to write, so the cut is planned on
            // the concatenation and the bytes are cut back along the element boundaries. A
            // rewriter that wrote the whole concatenation into the first element would collapse
            // the array -- which is a valid-looking document whose object graph this operation
            // silently rewrote. ADR 0029 §4.
            StreamId::Page => {
                let Some((contents, elements)) = self.page_contents(&page)? else {
                    // probe-allowed: a burrow invariant, not a judgement about the file
                    return Err(Error::Internal(
                        "pdf redaction: a page stream to rewrite on a page with no /Contents"
                            .to_owned(),
                    ));
                };
                let mine: Vec<Glyph> = self
                    .cut
                    .iter()
                    .filter(|glyph| glyph.source.form.is_none())
                    .cloned()
                    .collect();
                // THE COMBINED PASS: the glyph cuts and the carried text in one application,
                // because both are spans into these same bytes. See
                // `remove_glyphs_and_carried_text`.
                // COUNTED INTO A LOCAL, ADDED AFTER THE HANDLES GO. `page` borrows `self`,
                // so `self.dropped_carried_text += …` here borrows it twice. The count comes
                // back from the same walk that did the work; asking for it separately ran the
                // covering-span walk twice per stream.
                let (parts, dropped) = remove_glyphs_and_carried_text(
                    &contents,
                    None,
                    &mine,
                    &FormsReached::Named(&self.page_draws),
                    &self.page_properties,
                    &self.watch(),
                )?;
                if parts.len() != elements.len() {
                    // probe-allowed: a burrow invariant, not a judgement about the file
                    return Err(Error::Internal(
                        "pdf redaction: the splice returned a different number of streams than \
                         the page has elements"
                            .to_owned(),
                    ));
                }
                for (element, bytes) in elements.iter().zip(&parts) {
                    // WRITTEN BACK THROUGH THE TRAPPED VERB, with a null filter: the
                    // replacement is plain bytes, and a null `/Filter` is how "no filter" is
                    // said. qpdf re-compresses on write and sets `/Length` itself -- measured
                    // against 12.4.1 on the emitted file, not assumed.
                    element.replace_stream_data(bytes, &null, &null)?;
                }
                // NOTHING IS RETURNED FROM HERE. `codes_still_drawn` re-reads the document
                // through qpdf rather than any record this step keeps -- which is the point of
                // running it after the write. The field that used to hold these bytes claimed
                // otherwise in its own doc comment and was read by nobody; it is gone.
                dropped
            }
            StreamId::Object(id) => {
                let form = find_form(&resources, id)?;
                let mine: Vec<Glyph> = self
                    .cut
                    .iter()
                    .filter(|glyph| glyph.source.form == Some(id))
                    .cloned()
                    .collect();
                // THE SAME COMBINED PASS FOR A FORM. A form can both hold removed glyphs and
                // carry the span covering them -- `evade-actualtext-inside-a-form` is exactly
                // that -- so doing only the glyphs here would leave the text it replaces.
                let empty = BTreeSet::new();
                let draws = self.form_draws.get(&id).unwrap_or(&empty);
                // A FORM WITH NO RECORDED SCOPE RESOLVES NOTHING, and nothing is the cautious
                // answer: every named span in it then refuses as unresolved rather than passing.
                let unresolved = NamedProperties::default();
                let properties = self.form_properties.get(&id).unwrap_or(&unresolved);
                let (parts, dropped) = remove_glyphs_and_carried_text(
                    &crate::pdfsyntax::contents::Contents::concatenate(&[&form])?,
                    Some(id),
                    &mine,
                    &FormsReached::Named(draws),
                    properties,
                    &self.watch(),
                )?;
                let edited = parts.into_iter().next().ok_or_else(|| {
                    // probe-allowed: a burrow invariant, not a judgement about the file
                    Error::Internal(
                        "pdf redaction: the splice returned no stream for a form".to_owned(),
                    )
                })?;
                // NO TYPE CHECK HERE, and its absence is deliberate. `form_handle` returns
                // only entries whose `type_code()` is `STREAM` and otherwise refuses with
                // `form-vanished`, so a check here could not fire -- a review planted a
                // mutation of it and nothing could fail. An unreachable guard reads as
                // coverage and is not any.
                let target = form_handle(&resources, id)?;
                target.replace_stream_data(&edited, &null, &null)?;
                drop(target);
                dropped
            }
        };
        // `null` TOO. It is made beside the page now rather than from the document (#191), so it
        // borrows what `page` borrows, and it is released here instead of at the end of the
        // function. A release is a map erase on the document and changes nothing in it.
        drop(null);
        drop(resources);
        drop(page);
        // ADDED AFTER THE HANDLES GO, because `page` borrows `self` for the whole match.
        self.dropped_carried_text += dropped_here;
        Ok(())
    }

    fn codes_still_drawn(&mut self, redacted: &BTreeSet<usize>) -> Result<Vec<(u64, Vec<u32>)>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        // THE FINISHED CONTENT, which is why this step cannot run before the rewrites. The
        // `/ToUnicode` entry for a removed glyph IS the removed character; a font cut from a
        // snapshot taken part-way would keep entries for codes a later edit removed.
        //
        // **EVERY PAGE IN THE OPERATION, not just the edited one.** "No longer drawn" is a
        // fact about the document, and `cut_fonts` decides cuttability across the whole
        // `redacted` set — so reading one page's codes and cutting on the strength of the whole
        // set zeroed the widths of every code the OTHER redacted pages draw. Measured by a
        // code review on a four-page fixture with `redacted = {0,1,2,3}`: "AAAA" on pages 1
        // to 3 collapsed onto one origin, while the report said `cut: true, also_used_by: 0` —
        // that nothing outside the operation had been affected.
        //
        // The other pages are unedited, so every code they draw is still drawn. That is not a
        // special case; it is the same question asked of each page in turn.
        let mut drawn: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
        for at in self.pages_in_scope(redacted)? {
            // ONCE PER FONT AND ROUTE ON A PAGE, not once per glyph. A route is at most
            // `MAX_FORM_DEPTH` lookups, but a page may draw 200,000 glyphs, and a per-glyph
            // resolution is the shape a review measured at 58 s for 100 glyphs when resolution
            // was a search (#218, round 3). Per page, because resources are.
            let mut resolved: BTreeMap<ScopedFont, u64> = BTreeMap::new();
            self.deadline.checkpoint(self.clock.as_ref())?;
            // `pages_in_scope` yields only indices below the document's page count, which it
            // reads from the document itself. `page` checks again, and drains as this did.
            let page = self.document.page(at)?;
            let content = page.page_content()?;
            let resources = PageResources::of(&page)?;
            for glyph in &glyphs_in(&content, &resources, &self.watch())? {
                // KEYED BY THE FONT'S OBJECT, not its resource name: two names can mean one
                // object and one name can mean different objects on different pages, and font
                // surgery edits objects. `core/CLAUDE.md`'s identity rule, one level up.
                //
                // IN THE SCOPE THAT DREW IT. `font_object` searches the page's `/Font`
                // only, so an ordinary document whose form carries its own failed with
                // `font-missing` -- blaming the file for a one-scope lookup.
                let font = match resolved.get(&glyph.source.font) {
                    Some(&font) => font,
                    None => {
                        let font = pack(resources.font_in_scope(&glyph.source.font)?.object()?);
                        resolved.insert(glyph.source.font.clone(), font);
                        font
                    }
                };
                drawn.entry(font).or_default().insert(glyph.source.code);
            }
        }
        Ok(drawn
            .into_iter()
            .map(|(font, codes)| (font, codes.into_iter().collect()))
            .collect())
    }

    fn cut_fonts(
        &mut self,
        still_drawn: &[(u64, Vec<u32>)],
        redacted: &BTreeSet<usize>,
    ) -> Result<(Vec<FontOutcome>, Vec<crate::redact_verify::FontPath>)> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        let resources = PageResources::of(&page)?;

        let mut outcomes = Vec::new();
        let mut cut_paths = Vec::new();

        // EVERY FONT THE OPERATION DREW WITH, in whatever scope named it. This iterated the
        // page's `/Font` keys, so a font named only by a form's own `/Resources` was never
        // narrowed: measured, a form-local font's `/ToUnicode` still mapped both removed
        // characters in the output, and §6 missed it because `mapped_codes` enumerated the page
        // too. `/ToUnicode` IS the removed character in plain text.
        //
        // Deduplicated by **identity**, because two scopes may name one object and one name may
        // mean different objects in different scopes -- which is what name-based enumeration was
        // quietly assuming away.
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        // WHETHER A GLYPH DREW WITH IT, beside each font: a glyph's font resolved once for the
        // walk to place it, so failing to resolve it now is burrow disagreeing with itself and
        // propagates. A page `/Font` key is only a candidate -- one naming something that is not
        // a font dictionary is not a font to cut, and refusing it refused documents burrow
        // handles (#218's first design).
        let mut scoped: BTreeMap<ScopedFont, bool> = BTreeMap::new();
        for glyph in &self.cut {
            scoped.insert(glyph.source.font.clone(), true);
        }
        for name in font_names(&resources)? {
            scoped
                .entry(ScopedFont::on_page(name.plain().to_vec()))
                .or_insert(false);
        }

        for (scoped_font, drew) in &scoped {
            // PER FONT: a page may name 4,096, and narrowing one parses its `/ToUnicode` (up to
            // 65,536 entries) and rewrites its `/Widths`. Checked only on entry, 4,000 fonts
            // sharing one full-range `/ToUnicode` ran **36 s** against a 100 ms budget, from a
            // 500 KB file.
            self.deadline.checkpoint(self.clock.as_ref())?;
            let (font, path) = match resources.font_path_in_scope(scoped_font) {
                Ok(found) => found,
                // DEFENSIVE, AND UNWITNESSED: the walk placed the glyph along this route through
                // the same scoping, and nothing between the walk and here edits `/Resources`, so
                // no document reaches this arm. It fails closed rather than skip a cut font.
                Err(error) if *drew => return Err(error),
                Err(_) => continue,
            };
            // A DICTIONARY BY CONSTRUCTION: `font_path_in_scope` resolves only to one.
            let identity = font.object()?;
            // A DIRECT FONT HAS NO IDENTITY (#218, owner's decision 2026-09-28). qpdf reports
            // `(0, 0)` for every font dictionary written inline, so two of them are one font to
            // the dedupe below -- the first was narrowed and the second never touched, and the
            // sharing rule cannot say whether an object with no identity is shared. A security
            // review got that to return `Ok` with a removed character still mapped. Refused by
            // name: none of the golden corpus's documents has one.
            if identity == (0, 0) {
                return Err(Error::Unsupported(
                    "pdf redaction [direct-font]: a font dictionary written inline, which has no \
                     identity to tell it from another, so whether it is shared and whether it was \
                     narrowed cannot be established"
                        .to_owned(),
                ));
            }
            let packed = pack(identity);
            if !seen.insert(packed) {
                continue;
            }
            // ONE EXPRESSION FOR BOTH THE DECISION AND THE DISCLOSURE. `pages_outside` counts
            // the pages an edit to this font would reach -- including through the `/ToUnicode`,
            // `/Encoding` and `/Widths` it names, which can be indirect and shared. Asking
            // twice, once for a cuttable set and once for a count, is how the two stop
            // agreeing; see `FormUseCounts::pages_outside`.
            let outside = self.sharing.pages_outside(identity, redacted);

            if outside > 0 {
                // LEFT INTACT AND DISCLOSED. Editing it reflows every other page that uses it
                // -- corruption rather than leakage, and invisible to §6's read-back.
                outcomes.push(FontOutcome {
                    font: packed,
                    cut: false,
                    also_used_by: outside,
                });
                continue;
            }

            let keeps: BTreeSet<u32> = still_drawn
                .iter()
                .find(|(other, _)| *other == packed)
                .map(|(_, codes)| codes.iter().copied().collect())
                .unwrap_or_default();
            narrow_font(&font, &keeps)?;
            outcomes.push(FontOutcome {
                font: packed,
                cut: true,
                also_used_by: outside,
            });
            // WHERE IT WAS CUT, by names the writer keeps, for the read-back (#218).
            cut_paths.push(path);
        }
        Ok((outcomes, cut_paths))
    }

    fn strip_page_keys(&mut self) -> Result<()> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        // ADR 0029 §2's allowlist, shared with `prune` rather than written again: two lists
        // that are supposed to agree are two lists that can disagree.
        for key in crate::prune::page_keys_outside_the_allowlist(&page.unparse())? {
            page.remove_key(&Name::from_stripped(&key)?);
            page.drained()?;
        }
        let frame = Self::frame(&page)?;
        let region = self.region.to_content_space(&frame)?;
        let removed = remove_annotations_in(
            &page,
            &region,
            frame.rotate,
            &self.deadline,
            self.clock.as_ref(),
        )?;
        self.refuse_what_the_removal_leaves(&removed, &page)
    }

    fn write(&mut self) -> Result<Vec<u8>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let bytes = self.document.write()?;
        // A REPAIR AFTER THE OPEN IS REFUSED TOO, asked once, after the write and before any
        // byte leaves (#224, security reviews rounds 2 and 3). qpdf reads lazily: a font whose
        // `/Widths` held a stray `)` was repaired during the walk, and PDFium ended the array at
        // it and placed the glyphs otherwise -- `Ok` with the secret kept; a stream reached only
        // from the catalog, with a wrong `/Length`, was repaired during the write itself. qpdf's
        // warnings persist, so one check here sees both, and the bytes are dropped. (A second
        // check before the write refused nothing this one does not, and no test could pin it.)
        if self.document.repaired() {
            return Err(super::repaired_by_the_engine());
        }
        Ok(bytes)
    }
}

/// Remove every annotation whose `/Rect` intersects the region.
///
/// # This was missing, and it returned `Ok` over a rendered secret
///
/// `affected_streams` enumerates the page's content and the Form XObjects reached through its
/// `/Resources`. It never walks `/Annots → /AP → /N`, and `/Annots` is on §2's allowlist, so an
/// annotation's appearance stream drawing over the region survived untouched. A security review
/// measured it: a `/FreeText` annotation whose appearance showed `(SECRET) Tj`, a region
/// squarely over it, and the output still rendered SECRET — with the report saying the font was
/// cut and nothing retained.
///
/// It was worse than a no-op. `codes_still_drawn` walks page content, so the codes the
/// annotation drew were not in `keeps`, and font surgery zeroed **their** widths in the shared
/// font. The output both kept the secret and drew it overlapping.
///
/// ADR 0029 §3 already said what to do, in words: *"annotations whose `/Rect` intersects the
/// region — handle. Remove the annotation entirely; pruning its `/Contents` and keeping its
/// appearance is two chances to miss one."* This is that sentence, and the whole-annotation
/// removal is why there is no appearance-stream rewriting here.
///
/// # Backwards, and an unreadable `/Rect` is a refusal
///
/// [`PdfObject::erase_item`] renumbers, so a forward loop removing item 2 of 5 makes the old
/// item 3 the new item 2 and never examines it — the same defect `prune`'s `/Annots` walk was
/// written backwards to avoid, and for the same reason: a skipped annotation is one whose
/// appearance nobody looked at.
///
/// An annotation whose `/Rect` is absent or not four numbers is refused rather than kept: where
/// it sits is then unknown, and unknown is not "outside the region".
///
/// # And an annotation it keeps must draw inside its `/Rect` (#229)
///
/// Three refusals for every annotation kept: a NoRotate one on a turned page, one with an appearance
/// no `/BBox` bounds, and a text-markup one whose `/QuadPoints` reach outside its `/Rect`. Each is a
/// way PDFium draws outside the `/Rect`, measured. The walk reads the deadline at every annotation
/// and every appearance stream, and reads a shared `/AP` dictionary, state dictionary, stream or
/// `/QuadPoints` array once **per role** it is reached in: the code review of #229 measured 182.6 s
/// on a 616 KB file whose 5,000 annotations shared one `/AP`, when neither held.
///
/// # And what it removes leaves the file (#239)
///
/// An annotation taken out of `/Annots` was still written by qpdf when anything reached it -- a
/// margin Popup's `/Parent`, a reply's `/IRT`, an action on the catalogue -- with its `/Contents`
/// in the output bytes after an `Ok`. So its dependents on this page go with it (see
/// [`with_dependents`]), and a kept annotation loses a `/Popup` entry naming one. Returns what was
/// removed and what was edited, so the caller can refuse anything kept that still names either.
fn remove_annotations_in<O: PdfObject>(
    page: &O,
    region: &crate::pdfsyntax::geometry::Rect,
    rotate: u16,
    deadline: &Deadline,
    clock: &dyn Clock,
) -> Result<Removed> {
    const ANNOTS: Name = Name::literal(b"/Annots\0");
    const RECT: Name = Name::literal(b"/Rect\0");

    let annots = page.key(&ANNOTS);
    page.drained()?;
    if annots.type_code() != object_type::ARRAY {
        return Ok(Removed {
            annotations: BTreeSet::new(),
            popups: BTreeSet::new(),
            edited: BTreeSet::new(),
            annots_erased: None,
        });
    }
    let length = annots.array_len();
    if length > DIFFERENCES_CEILING {
        return Err(Error::Unsupported(
            "pdf redaction [annots-too-long]: an /Annots array longer than burrow will examine"
                .to_owned(),
        ));
    }
    // ONE READ OF EACH ANNOTATION: where it sits, what it is, and what it names (#239).
    let mut entries = Vec::new();
    for at in 0..length {
        deadline.checkpoint(clock)?;
        let annotation = annots.array_item(at);
        if annotation.type_code() != object_type::DICTIONARY {
            // Not an annotation. `prune`'s walk found these too, and the container type check
            // is what stops each one retaining a qpdf warning.
            continue;
        }
        let rect = annotation.key(&RECT);
        annotation.drained()?;
        // READ AS PDFIUM READS IT, and as the page frame is (#224, security reviews rounds 3
        // and 4): an array of exactly four items, each one number both readers agree on -- by
        // the one box reader the frame and an appearance's `/BBox` use too. The numbers used to
        // be scanned out of the `/Rect`'s text, so `[[200 0] 400 120 []]` was 200 0 400 120 here
        // -- clear of the region -- and 0 400 120 0 to PDFium, over it; and a top edge of 2^32
        // over a bottom of 380 was 380 upwards here and 0..380 to PDFium, which reads a whole
        // number outside 32 bits as 0. Each kept the annotation, `Ok`. Normalised, because a
        // `/Rect`'s corners come in either order (PDF 32000-1 §12.5.2), and an un-normalised
        // rectangle compares as empty against every region.
        let box_of = match super::frame::four_numbers(&rect) {
            super::frame::BoxReading::Box(box_of) => box_of,
            super::frame::BoxReading::NotFour => {
                return Err(Error::Malformed(
                    "pdf redaction [annotation-rect]: an annotation whose /Rect is not four \
                     numbers, so where it draws is unknown"
                        .to_owned(),
                ));
            }
            super::frame::BoxReading::OutOfRange => {
                return Err(Error::Unsupported(
                    "pdf redaction [annotation-rect]: an annotation whose /Rect is larger \
                     than any reader agrees on, so where it draws is unknown"
                        .to_owned(),
                ));
            }
        };
        entries.push(Entry {
            at,
            identity: identity_of(&annotation)?,
            over: box_of.intersects(region),
            rect: box_of,
            parent: identity_of(&annotation.key(&PARENT))?,
            in_reply_to: identity_of(&annotation.key(&IRT))?,
            popup: popup_of(&annotation)?,
        });
        annotation.drained()?;
    }

    let removed = with_dependents(&entries, deadline, clock)?;
    let gone: BTreeSet<(c_int, c_int)> = entries
        .iter()
        .zip(&removed)
        .filter_map(|(entry, &out)| if out { entry.identity } else { None })
        .collect();
    // A REMOVED ANNOTATION'S `/Popup` MUST READ: one that is neither a dictionary nor absent
    // names something the walk cannot follow (#239's fourth review -- `/Popup [7 0 R]` wrote the
    // Popup out after an `Ok`). A kept one's is its own business.
    if entries
        .iter()
        .zip(&removed)
        .any(|(entry, &out)| out && entry.popup == Popup::Unreadable)
    {
        return Err(popup_unreadable());
    }
    let popups: BTreeSet<(c_int, c_int)> = entries
        .iter()
        .zip(&removed)
        .filter_map(|(entry, &out)| if out { entry.popup.at() } else { None })
        .collect();

    let mut walk = AppearanceWalk {
        deadline,
        clock,
        checked: Checked::default(),
        quads: BTreeMap::new(),
    };
    let mut edited = BTreeSet::new();
    for (entry, &out) in entries.iter().zip(&removed).rev() {
        deadline.checkpoint(clock)?;
        let annotation = annots.array_item(entry.at);
        if out {
            // AFTER THE READ, NOT DURING IT: `erase_item` renumbers, and walking backwards over
            // the recorded positions erases each one where it was read.
            annots.erase_item(entry.at);
            annots.drained()?;
            continue;
        }
        // A KEPT ANNOTATION WHOSE POPUP WAS REMOVED keeps itself and loses the popup: the popup
        // sat over the region, which is the disclosure's own sentence, and a `/Popup` left naming
        // it would write it out again (#239).
        if entry.popup.at().is_some_and(|popup| gone.contains(&popup)) {
            annotation.remove_key(&POPUP);
            annotation.drained()?;
            edited.extend(entry.identity);
        }
        // KEPT, SO WHERE IT DRAWS MUST BE ITS `/Rect` (#229). The test above assumes an
        // annotation draws only inside its `/Rect`, and PDFium does not always keep it there.
        refuse_no_rotate_on_a_turned_page(&annotation, rotate)?;
        walk.refuse_quads_outside_the_rect(&annotation, &entry.rect)?;
        walk.refuse_unbounded_appearances(&annotation)?;
    }
    // THE ARRAY ITSELF, where it is its own object and something was erased from it (#240).
    let annots_erased = if removed.iter().any(|&out| out) {
        identity_of(&annots)?
    } else {
        None
    };
    Ok(Removed {
        annotations: gone,
        popups,
        edited,
        annots_erased,
    })
}

/// One annotation of a page, as `remove_annotations_in` read it.
struct Entry {
    /// Its position in `/Annots` when read.
    at: c_int,
    /// Its object identity, or `None` for a direct dictionary, which nothing else can name.
    identity: Option<(c_int, c_int)>,
    /// Whether its `/Rect` meets the region.
    over: bool,
    rect: crate::pdfsyntax::geometry::Rect,
    /// What its `/Parent` and `/IRT` name, where each is an indirect object.
    parent: Option<(c_int, c_int)>,
    in_reply_to: Option<(c_int, c_int)>,
    /// What its `/Popup` names; see [`popup_of`].
    popup: Popup,
}

/// What an annotation's `/Popup` names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Popup {
    /// No `/Popup`, or a chain of inline ones that ends without naming an object.
    None,
    /// The first object with an identity along the chain.
    At((c_int, c_int)),
    /// A `/Popup` that is neither a dictionary, an object, nor absent -- an array, say -- or a chain
    /// of inline ones past the cap. (A `/Popup` that is an object but not a dictionary is caught
    /// where the walk reads that object.) Refused where the annotation is removed, as
    /// `[annotation-popup-unreadable]`: what it names cannot be followed.
    Unreadable,
}

impl Popup {
    fn at(self) -> Option<(c_int, c_int)> {
        match self {
            Self::At(identity) => Some(identity),
            Self::None | Self::Unreadable => None,
        }
    }
}

/// The refusal for a removed annotation whose `/Popup` names something the walk cannot follow --
/// in a form that is not a dictionary, or past [`MAX_DIRECT_POPUPS`] Popups written inline.
fn popup_unreadable() -> Error {
    Error::Unsupported(
        "pdf redaction [annotation-popup-unreadable]: an annotation the region removes names its \
         Popup in a form burrow cannot follow, so whether anything kept still names it is unknown"
            .to_owned(),
    )
}

/// What a page's annotation pass did: every removed annotation that has an identity, and every
/// kept one with an identity whose `/Popup` entry it removed.
pub(super) struct Removed {
    pub(super) annotations: BTreeSet<(c_int, c_int)>,
    /// The Popup each removed annotation names -- **a direct one's too**, which has no identity of
    /// its own but whose Popup does (#239's second specification review: seeded from identities
    /// alone, a direct annotation's Popup was never followed, and was written out after an `Ok`
    /// when the catalogue or another page named it).
    pub(super) popups: BTreeSet<(c_int, c_int)>,
    pub(super) edited: BTreeSet<(c_int, c_int)>,
    /// The page's `/Annots` array, where it is its own object and the pass erased from it (#240).
    /// Erasing from it changes it wherever else it is named, so nothing but this page may name it.
    pub(super) annots_erased: Option<(c_int, c_int)>,
}

const PARENT: Name = Name::literal(b"/Parent\0");
const IRT: Name = Name::literal(b"/IRT\0");
const POPUP: Name = Name::literal(b"/Popup\0");

/// The Popup `annotation` names, as the first object with an identity along its `/Popup` chain:
/// a Popup written directly is followed to its own `/Popup` (#239's third specification review --
/// stopping at the direct one let the chain past it be written out after an `Ok`). `None` where
/// the chain ends without one, and [`Popup::Unreadable`] where it reaches anything else -- or more
/// than [`MAX_DIRECT_POPUPS`] Popups written inline, the annotation's own among them.
fn popup_of<O: PdfObject>(annotation: &O) -> Result<Popup> {
    let mut popup = annotation.key(&POPUP);
    // `..=`: up to `MAX_DIRECT_POPUPS` inline Popups in all, the annotation's own among them, and
    // one more slot for the reference that must close the chain.
    for _ in 0..=MAX_DIRECT_POPUPS {
        if let Some(identity) = identity_of(&popup)? {
            return Ok(Popup::At(identity));
        }
        match popup.type_code() {
            object_type::DICTIONARY => {}
            object_type::NULL => return Ok(Popup::None),
            _ => return Ok(Popup::Unreadable),
        }
        popup = popup.key(&POPUP);
    }
    // PAST THE CAP, UNREADABLE rather than an error of its own: a chain this deep is what the
    // walk cannot follow, which refuses only where the annotation is removed.
    Ok(Popup::Unreadable)
}

/// How many Popups written inline, the annotation's own `/Popup` among them, `popup_of` follows.
const MAX_DIRECT_POPUPS: usize = 32;

/// The identity of `object`, or `None` where it is direct and so has none.
fn identity_of<O: PdfObject>(object: &O) -> Result<Option<(c_int, c_int)>> {
    let identity = object.object()?;
    Ok((identity != (0, 0)).then_some(identity))
}

/// Which of a page's annotations go: each whose `/Rect` meets the region, and then, until none is
/// left, each that depends on one that goes (#239).
///
/// **A dependent goes with what it depends on** (owner, 2026-10-01): a Popup whose `/Parent` is
/// removed, a reply whose `/IRT` is, and the Popup a removed annotation names as its `/Popup`.
/// Removing it is part of removing its parent, so the disclosure that annotations over the region
/// are removed covers it. Kept, it was worse than an orphan: qpdf writes every object something
/// reaches, so a margin Popup's `/Parent` wrote the removed annotation -- its `/Contents` and its
/// appearance -- into the output, measured `Ok` with both strings in the bytes. A reply to a reply
/// goes too: the closure runs to a fixpoint, over an index built once, so it is linear in the
/// page's annotations.
fn with_dependents(entries: &[Entry], deadline: &Deadline, clock: &dyn Clock) -> Result<Vec<bool>> {
    let mut by_identity: BTreeMap<(c_int, c_int), usize> = BTreeMap::new();
    let mut dependents: BTreeMap<(c_int, c_int), Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Some(identity) = entry.identity {
            by_identity.insert(identity, index);
        }
        for named in [entry.parent, entry.in_reply_to].into_iter().flatten() {
            dependents.entry(named).or_default().push(index);
        }
    }
    let mut removed: Vec<bool> = entries.iter().map(|entry| entry.over).collect();
    let mut queue: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.over.then_some(index))
        .collect();
    while let Some(index) = queue.pop() {
        deadline.checkpoint(clock)?;
        let Some(entry) = entries.get(index) else {
            continue;
        };
        let mut next: Vec<usize> = entry
            .identity
            .and_then(|identity| dependents.get(&identity))
            .cloned()
            .unwrap_or_default();
        if let Some(popup) = entry.popup.at().and_then(|popup| by_identity.get(&popup)) {
            next.push(*popup);
        }
        for dependent in next {
            if let Some(slot) = removed.get_mut(dependent)
                && !*slot
            {
                *slot = true;
                queue.push(dependent);
            }
        }
    }
    Ok(removed)
}

/// The annotation flag that keeps an annotation upright when the page is turned
/// (PDF 32000-1 §12.5.3, bit 5).
const NO_ROTATE: i64 = 16;

/// Refuse a NoRotate annotation on a page whose effective `/Rotate` -- the frame's, inherited and
/// normalised -- is not 0 (#229).
///
/// PDFium turns such an appearance about the `/Rect`'s corner, so it draws outside the `/Rect`:
/// measured `Ok` with 1,800 dark pixels of its ink in a region beside the `/Rect` that the
/// `/Rect` does not meet. Refused rather than modelled, by the owner's decision: a second
/// geometry to keep in step with PDFium's is the residue §6 already records for #111. 0 of 99
/// real documents (30 with annotations) and 0 of 99 fixtures have the shape.
///
/// An `/F` that is not an integer is read as one with every flag set: what PDFium makes of it is
/// unmeasured, so on a turned page it refuses.
fn refuse_no_rotate_on_a_turned_page<O: PdfObject>(annotation: &O, rotate: u16) -> Result<()> {
    const F: Name = Name::literal(b"/F\0");
    if rotate == 0 {
        return Ok(());
    }
    let flags = annotation.key(&F);
    let no_rotate = match flags.type_code() {
        object_type::NULL => false,
        object_type::INTEGER => flags.integer_value() & NO_ROTATE != 0,
        _ => true,
    };
    if no_rotate {
        return Err(Error::Unsupported(
            "pdf redaction [annotation-no-rotate]: an annotation kept upright on a turned page, \
             which the renderer turns about a corner of its rectangle, so where it draws is not \
             where its rectangle is"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The appearance kinds an annotation can draw with: normal, down and rollover.
const APPEARANCES: [Name; 3] = [
    Name::literal(b"/N\0"),
    Name::literal(b"/D\0"),
    Name::literal(b"/R\0"),
];

/// The appearance walk over the annotations a page keeps: the deadline, and what it has checked.
struct AppearanceWalk<'d> {
    deadline: &'d Deadline,
    clock: &'d dyn Clock,
    /// What has already been checked, by object identity, **one set per role** -- so a shared
    /// `/AP` is walked once however many annotations name it. A direct object has no identity,
    /// `(0, 0)`, and is walked each time; its cost is its own bytes.
    ///
    /// Per role because the roles do not read the same keys: an `/AP` dictionary is read for
    /// `/N`, `/D` and `/R`, a state dictionary for every key. One set for all three let an object
    /// checked as an `/AP` be skipped when reached as a state dictionary, so its other states were
    /// never read: an `/AP` naming itself as its `/N`, or one annotation's `/AP` that is another's
    /// state dictionary, kept an unbounded appearance, `Ok`, with 654 dark pixels of its ink in the
    /// region -- found by both reviews of #229's memo, which introduced it.
    checked: Checked,
    /// A shared `/QuadPoints` array's extent, read once: what it is compared with is each
    /// annotation's own `/Rect`, so the extent is memoised and the verdict is not. `None` is an
    /// array no reader agrees on, refused whatever the `/Rect`.
    quads: BTreeMap<(c_int, c_int), Option<Extent>>,
}

/// One set of checked object identities per role an object can be reached in.
#[derive(Default)]
struct Checked {
    appearances: BTreeSet<(c_int, c_int)>,
    states: BTreeSet<(c_int, c_int)>,
    streams: BTreeSet<(c_int, c_int)>,
}

/// The least and greatest x and y of a `/QuadPoints` array.
#[derive(Clone, Copy)]
struct Extent {
    left: f64,
    right: f64,
    bottom: f64,
    top: f64,
}

#[cfg(test)]
thread_local! {
    /// How many appearance dictionaries, state dictionaries, streams and `/QuadPoints` arrays
    /// were read rather than recognised as checked, in that order.
    static WALKED: core::cell::Cell<[usize; 4]> = const { core::cell::Cell::new([0; 4]) };
}

/// How many appearance dictionaries, state dictionaries, streams and `/QuadPoints` arrays this
/// thread has read: the witness that each memo holds.
#[cfg(test)]
pub(crate) fn appearance_objects_walked() -> [usize; 4] {
    WALKED.with(core::cell::Cell::get)
}

/// Count one read of the role at `index`, under test.
#[cfg_attr(
    not(test),
    expect(unused_variables, reason = "counted under test only")
)]
fn count_walked(index: usize) {
    #[cfg(test)]
    WALKED.with(|walked| {
        let mut counts = walked.get();
        if let Some(count) = counts.get_mut(index) {
            *count += 1;
        }
        walked.set(counts);
    });
}

/// Whether `object` was already checked in `set`; recorded as checked if not. A direct object
/// never is.
fn already_checked<O: PdfObject>(set: &mut BTreeSet<(c_int, c_int)>, object: &O) -> Result<bool> {
    let identity = object.object()?;
    if identity == (0, 0) {
        return Ok(false);
    }
    Ok(!set.insert(identity))
}

/// The text-markup annotations PDFium draws from their `/QuadPoints` rather than their `/Rect`.
const MARKUP: [&[u8]; 4] = [b"Highlight", b"Underline", b"Squiggly", b"StrikeOut"];

impl AppearanceWalk<'_> {
    /// Refuse a kept text-markup annotation whose `/QuadPoints` reach outside its `/Rect` (#229).
    ///
    /// PDFium fits such an annotation's appearance into the box around its `/QuadPoints` rather
    /// than its `/Rect` in some cases -- one is a private `/PDFIUM_HasGeneratedAP` key a file can
    /// carry, which makes PDFium draw the file's own appearance there; another is a markup
    /// annotation with no `/AP`, whose appearance PDFium builds at the quadrilaterals. Measured: a
    /// Highlight whose `/Rect` misses the region and whose `/QuadPoints` cover it, carrying that
    /// key, left 1,304 dark pixels of its own appearance in the region after an `Ok`. Keyed on the
    /// declared structure -- quadrilaterals outside the rectangle -- rather than on any one private
    /// key, so a key PDFium adds later is covered too. A `/QuadPoints` that is not an array of
    /// numbers both readers agree on is refused the same way: where it sits is then unknown. So is
    /// a count that is not a multiple of eight whose numbers leave the `/Rect`, although PDFium
    /// ignores an incomplete quadrilateral: a harmless over-refusal. Compared exactly, and its
    /// false-refusal rate is **unmeasured** (#242): highlights from PDFium's own API carry no `/Rect`
    /// and are refused before this rule, and with one set to the quads' own numbers none can fail.
    /// An error there is a refusal, not a leak.
    ///
    /// **A `/Subtype` that is not a name is checked as markup.** PDFium reads the subtype as a byte
    /// string, so `(Highlight)` -- or the same in hex -- is a Highlight to it; reading names only
    /// kept one, `Ok`, with 1,304 dark pixels in the region (#229's reviews). Only an absent
    /// `/Subtype` is not markup.
    fn refuse_quads_outside_the_rect<O: PdfObject>(
        &mut self,
        annotation: &O,
        rect: &crate::pdfsyntax::geometry::Rect,
    ) -> Result<()> {
        const SUBTYPE: Name = Name::literal(b"/Subtype\0");
        const QUADPOINTS: Name = Name::literal(b"/QuadPoints\0");
        let refusal = || {
            Error::Unsupported(
                "pdf redaction [annotation-quads-outside-rect]: a text-markup annotation whose \
                 quadrilaterals reach outside its rectangle, where the renderer may draw it"
                    .to_owned(),
            )
        };
        let subtype = annotation.key(&SUBTYPE);
        let markup = match subtype.type_code() {
            object_type::NULL => false,
            object_type::NAME => MARKUP.contains(&subtype.name()?.plain()),
            _ => true,
        };
        if !markup {
            return Ok(());
        }
        let quads = annotation.key(&QUADPOINTS);
        match quads.type_code() {
            object_type::NULL => return Ok(()),
            object_type::ARRAY => {}
            _ => return Err(refusal()),
        }
        // NO DEADLINE READ HERE: the loop over annotations reads it before each, and each reads at
        // most one extent.
        let identity = quads.object()?;
        let extent = match self.quads.get(&identity) {
            Some(extent) if identity != (0, 0) => *extent,
            _ => {
                let extent = extent_of(&quads)?;
                if identity != (0, 0) {
                    self.quads.insert(identity, extent);
                }
                extent
            }
        };
        let Some(extent) = extent else {
            return Err(refusal());
        };
        if extent.left < rect.left
            || extent.right > rect.right
            || extent.bottom < rect.bottom
            || extent.top > rect.top
        {
            return Err(refusal());
        }
        Ok(())
    }

    /// Refuse an annotation any of whose appearance streams -- `/N`, `/D`, `/R`, and every state
    /// under each -- has a `/BBox` that is not four numbers enclosing an area (#229).
    ///
    /// PDFium fits an appearance into the `/Rect` through its `/BBox`; with none it only moves the
    /// appearance to the `/Rect`'s corner, and nothing clips it there. Measured: an appearance with
    /// no `/BBox` drawing 290 points above its `/Rect` left 654 dark pixels of ink in the region
    /// after an `Ok`; the same with `/BBox [0 0 100 20]`, 0. Read by the page frame's box reader,
    /// plus an area: a degenerate `/BBox` gives the fit no scale. Every state is checked, not the
    /// one `/AS` selects, because a viewer may draw any of them -- `/D` while pressed, `/R` under
    /// the pointer.
    fn refuse_unbounded_appearances<O: PdfObject>(&mut self, annotation: &O) -> Result<()> {
        const AP: Name = Name::literal(b"/AP\0");
        let appearances = annotation.key(&AP);
        if appearances.type_code() != object_type::DICTIONARY {
            return Ok(());
        }
        if already_checked(&mut self.checked.appearances, &appearances)? {
            return Ok(());
        }
        count_walked(0);
        for kind in &APPEARANCES {
            let entry = appearances.key(kind);
            match entry.type_code() {
                object_type::STREAM => self.refuse_unbounded(&entry)?,
                object_type::DICTIONARY => {
                    if already_checked(&mut self.checked.states, &entry)? {
                        continue;
                    }
                    count_walked(1);
                    // CAPPED BY THE KEY READER, which refuses a dictionary past its own `MAX_KEYS`
                    // rather than returning a short list.
                    let states = crate::pdfsyntax::dict::top_level_keys(&entry.unparse())?;
                    for state in states {
                        let stream = entry.key(&Name::from_stripped(&state)?);
                        if stream.type_code() == object_type::STREAM {
                            self.refuse_unbounded(&stream)?;
                        }
                    }
                }
                _ => {}
            }
        }
        appearances.drained()
    }

    /// Refuse one appearance stream whose `/BBox` does not bound it, reading the deadline first.
    fn refuse_unbounded<O: PdfObject>(&mut self, stream: &O) -> Result<()> {
        const BBOX: Name = Name::literal(b"/BBox\0");
        self.deadline.checkpoint(self.clock)?;
        if already_checked(&mut self.checked.streams, stream)? {
            return Ok(());
        }
        count_walked(2);
        let bounded = match super::frame::four_numbers(&stream.stream_dict().key(&BBOX)) {
            super::frame::BoxReading::Box(b) => b.right > b.left && b.top > b.bottom,
            super::frame::BoxReading::NotFour | super::frame::BoxReading::OutOfRange => false,
        };
        if bounded {
            return Ok(());
        }
        Err(Error::Unsupported(
            "pdf redaction [annotation-appearance-unbounded]: an annotation whose appearance has \
             no bounding box the renderer fits to its rectangle, so where it draws is not where \
             its rectangle is"
                .to_owned(),
        ))
    }
}

/// The extent of a `/QuadPoints` array: x at even indices, y at odd, as the eight numbers of each
/// quadrilateral alternate. `None` for one no reader agrees on -- longer than the ceiling, or an
/// item that is not one number both readers read alike. An empty array extends nowhere, and
/// compares as inside every `/Rect`.
fn extent_of<O: PdfObject>(quads: &O) -> Result<Option<Extent>> {
    count_walked(3);
    let count = quads.array_len();
    if count > DIFFERENCES_CEILING {
        return Ok(None);
    }
    let mut extent = Extent {
        left: f64::INFINITY,
        right: f64::NEG_INFINITY,
        bottom: f64::INFINITY,
        top: f64::NEG_INFINITY,
    };
    for at in 0..count {
        let Reading::Number(value) = super::frame::reading_of(&quads.array_item(at)) else {
            return Ok(None);
        };
        if at % 2 == 0 {
            extent.left = extent.left.min(value);
            extent.right = extent.right.max(value);
        } else {
            extent.bottom = extent.bottom.min(value);
            extent.top = extent.top.max(value);
        }
    }
    quads.drained()?;
    Ok(Some(extent))
}

/// An object identity packed into the `u64` `Form::id` and `FontOutcome::font` use.
pub(super) const fn pack(identity: (core::ffi::c_int, core::ffi::c_int)) -> u64 {
    // `unsigned_abs`, not `as`: this crate denies sign-losing casts, and an object number is
    // never negative in a document qpdf opened. The same expression as
    // `PageResources::font_object`, because the two pack the same thing and a packing that
    // disagreed with itself would make one font look like two.
    ((identity.0.unsigned_abs() as u64) << 16) | (identity.1.unsigned_abs() as u64 & 0xffff)
}

/// Refuse when the region reaches a `/Contents` element that something else also references.
///
/// # The hazard, and why the form rule did not cover it
///
/// A glyph drawn by the page rather than by a Form XObject has `source.form == None`, and
/// [`check_form_sharing`] only counts forms. Two pages pointing at one `/Contents` object is
/// legal and ordinary — this crate's own `inheriting_document()` fixture builds one, because it
/// was the shortest way to write a two-page document. Editing that stream removes the text from
/// **both** pages, and §6's read-back is clean on the page it was given. ADR 0029 carried this
/// as a named gap; this is it closed.
///
/// # Per element, and that is the whole difficulty
///
/// A page-level rule would be a line shorter and wrong in both directions. **Under-detecting**
/// — asking only whether the `/Contents` *array object* is shared — edits a shared element in
/// place. **Over-detecting** — refusing any page one of whose elements is shared — refuses a
/// document whose shared element is a letterhead the region never reaches, which is a common
/// shape and a page that could have been served.
///
/// So the question is asked of the element the cut actually lands in:
/// [`Contents::locate`](crate::pdfsyntax::contents::Contents::locate) maps the glyph's operation
/// offset back to an element index, and the handle at that index is the object to count.
///
/// # A repeat inside one page counts
///
/// `/Contents [5 0 R 5 0 R]` is one page referencing one object twice. The concatenation holds
/// that text twice and an edit written back to the object applies at both positions, so a rule
/// asking "does another *page* use this?" answers no and is wrong. The count is of references.
///
/// # Errors
///
/// [`Error::Malformed`] naming `shared-contents`. [`Error::Internal`] naming
/// `contents-offset` for a glyph whose operation offset does not fall in any element — a
/// burrow invariant failing rather than a document being unusual, which is why the variant is
/// `Internal` and not `Malformed`. It is refused rather than skipped either way.
fn check_contents_sharing<O: PdfObject>(
    cut: &[Glyph],
    contents: &crate::pdfsyntax::contents::Contents,
    elements: &[O],
    sharing: &FormUseCounts,
    page: usize,
) -> Result<()> {
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for glyph in cut {
        if glyph.source.form.is_some() {
            continue;
        }
        let Some((at, _)) = contents.locate(glyph.source.operation.0) else {
            // probe-allowed: a burrow invariant, not a judgement about the file
            return Err(Error::Internal(
                "pdf redaction [contents-offset]: a glyph whose operation is not inside any \
                 /Contents element"
                    .to_owned(),
            ));
        };
        if !seen.insert(at) {
            continue;
        }
        let Some(element) = elements.get(at) else {
            // probe-allowed: a burrow invariant, not a judgement about the file
            return Err(Error::Internal(
                "pdf redaction [contents-offset]: an element index the page does not have"
                    .to_owned(),
            ));
        };
        let identity = element.object()?;
        // THE TOTAL, by any route. Asking `content_references` alone was a leak: an object
        // reached once as this page's `/Contents` and once as a form elsewhere had a count of
        // one in each map, and both rules passed. See `FormUseCounts::total_references`.
        let references = sharing.total_references(identity);
        if references > 1 {
            let others = sharing.content_pages_of(identity);
            let elsewhere = others.iter().filter(|other| **other != page).count();
            return Err(Error::Malformed(format!(
                "pdf redaction [shared-contents]: the region reaches a content stream this \
                 document references {references} times, across {elsewhere} other page(s); \
                 editing it would remove text from a page that was not selected"
            )));
        }
    }
    Ok(())
}

/// Refuse if any Type 3 font the page draws with has a procedure that shows text.
///
/// # Scoped to the page's fonts, not to the glyphs the region reached
///
/// It was scoped to the cut glyphs, and a security review measured what that missed. The walk
/// boxes a Type 3 glyph by its `/Widths` advance and its `/FontBBox`; it does not descend into
/// the procedure. So a procedure that draws its ink **somewhere else entirely** — a
/// `/FontBBox [0 0 10 10]` glyph whose content stream does `30 -100 Td (SECRET) Tj` — is in no
/// region's `cut`, the check never ran, and the operation returned `Ok` over a page that still
/// rendered the secret. Input and output were byte-identical when rendered.
///
/// A Type 3 glyph's real extent is not knowable from the font dictionary, so the scope cannot
/// be "the glyphs the region reached": that set is computed from the boxes that are wrong. It
/// has to be every Type 3 font the page draws with at all.
///
/// # Every procedure, and the name resolution is gone
///
/// It also resolved the specific `/CharProcs` entry through `/Encoding /Differences`, taking
/// the **first** assignment for a code. Readers take the last, so `[5 /g 5 /secret]` resolved
/// to the harmless procedure while poppler rendered the other one — and because a resolved
/// name absent from `/CharProcs` matched nothing, it then scanned **zero** procedures while
/// the doc comment claimed it over-refused.
///
/// Both defects are in the resolution, so the resolution is gone. Every procedure of every
/// Type 3 font the page draws with is scanned. That over-refuses, which is the direction that
/// does not leak, and it costs nothing measurable: the redaction corpus's Type 3 documents
/// still redact, because a bitmap glyph's procedure draws an inline image and shows no text.
///
/// # Each procedure once, and the deadline read per procedure
///
/// `/CharProcs` maps names to streams, and nothing stops four thousand names mapping to one
/// stream. Scanned per name, a 52 KB file of 4,000 keys over one 100,000-operation procedure
/// took **9.9 s** against a 100 ms budget -- a security review found it, and it was measured.
/// So procedures are scanned once per object identity, and `watch` is read inside every scan.
///
/// # What it returns
///
/// The fonts, of those drawn, with a procedure that draws an inline image inside the font's box
/// (#125): the caller refuses a region that reaches one of their glyphs. A procedure is scanned
/// once, and the extent of its images cached with it; that extent is judged against the `/FontBBox`
/// of **every** font that names the procedure, cached or not. The first version cached the verdict
/// instead, so a second font sharing the procedure with a smaller box inherited the first font's
/// "inside" -- `Ok` over 6,000 dark pixels, measured by #125's code review.
fn check_type_three<O: PdfObject>(
    resources: &PageResources<O>,
    drawn: &BTreeSet<ScopedFont>,
    watch: &Watch<'_>,
) -> Result<BTreeSet<ScopedFont>> {
    let mut scans = TypeThreeScans::default();
    // EACH FONT OBJECT ONCE, whatever names it is drawn under (#125's security review): 4,000 names
    // for one Type 3 font re-parsed its `/CharProcs` 4,000 times and read the deadline on none of
    // the cache hits -- 7.4 s against a 500 ms budget, the review's measurement. The verdict is the
    // font's, so it is computed once and given to every name.
    let mut fonts_judged: BTreeMap<u64, bool> = BTreeMap::new();
    let mut image_fonts = BTreeSet::new();
    for font_name in drawn {
        watch.tick()?;
        let font = resources.font_in_scope(font_name)?;
        // A FONT WRITTEN INLINE has the identity `(0, 0)`, shared by every direct object, so it is
        // judged every time rather than cached: two inline fonts must never share a verdict. This
        // is load-bearing, not belt and braces: `check_type_three` runs inside `affected_streams`,
        // BEFORE `cut_fonts` raises `[direct-font]`, so an inline Type 3 font reaches here.
        let object = font.object()?;
        let font_identity = (object != (0, 0)).then(|| pack(object));
        if let Some(&draws_image) = font_identity.and_then(|id| fonts_judged.get(&id)) {
            if draws_image {
                image_fonts.insert(font_name.clone());
            }
            continue;
        }
        let draws_image = judge_type_three_font(&font, &mut scans, watch)?;
        if let Some(id) = font_identity {
            fonts_judged.insert(id, draws_image);
        }
        if draws_image {
            image_fonts.insert(font_name.clone());
        }
    }
    Ok(image_fonts)
}

/// What `check_type_three` has already learned, across fonts: each procedure's image extent, and
/// each `/CharProcs` dictionary's image extents.
#[derive(Default)]
struct TypeThreeScans {
    /// Per procedure stream: the extent of the image it draws, if any.
    procedures: BTreeMap<u64, Option<Rect>>,
    /// Per indirect `/CharProcs` dictionary: the extents of every image its procedures draw.
    /// SEPARATE FONT OBJECTS SHARING ONE (#125's third security review): 4,000 fonts over one
    /// 4,000-key `/CharProcs` re-read every key per font, 20.5 s under the default budget -- the
    /// font memo cannot help when the identities differ. The extents are the dictionary's; each
    /// font's box is still judged against them, per font.
    char_procs: BTreeMap<u64, Vec<Rect>>,
}

/// One Type 3 font, judged once: whether any procedure it names draws an image (inside its box,
/// or this refuses), after every procedure has passed `check_type_three_procedure` -- cached per
/// procedure and per `/CharProcs` across fonts, with the deadline read on every key.
fn judge_type_three_font<O: PdfObject>(
    font: &O,
    scans: &mut TypeThreeScans,
    watch: &Watch<'_>,
) -> Result<bool> {
    const TYPE_THREE: Name = Name::literal(b"/Type3\0");
    const CHAR_PROCS: Name = Name::literal(b"/CharProcs\0");
    const FONT_BBOX: Name = Name::literal(b"/FontBBox\0");
    const MATRIX: Name = Name::literal(b"/Matrix\0");

    if super::resources::subtype_of(font)? != Some(TYPE_THREE) {
        return Ok(false);
    }
    let procs = font.key(&CHAR_PROCS);
    if procs.type_code() != object_type::DICTIONARY {
        return Ok(false);
    }
    let font_bbox = super::resources::rect_of(&font.key(&FONT_BBOX))?;
    // A DIRECT `/CharProcs` has the shared identity `(0, 0)`, so it is never cached.
    let procs_object = procs.object()?;
    let procs_identity = (procs_object != (0, 0)).then(|| pack(procs_object));
    let extents = if let Some(cached) = procs_identity.and_then(|id| scans.char_procs.get(&id)) {
        cached.clone()
    } else {
        let mut extents = Vec::new();
        for key in crate::pdfsyntax::dict::top_level_keys(&procs.unparse())? {
            watch.tick()?;
            let entry = procs.key(&Name::from_stripped(&key)?);
            if entry.type_code() != object_type::STREAM {
                continue;
            }
            // A `/MATRIX` ON THE PROCEDURE IS REFUSED (#125's security review). PDFium reads a glyph
            // procedure as it reads a form, so a `/Matrix` on its stream moves everything it draws
            // before the procedure's own `cm` -- an image this scan judged inside its box drew
            // 6,000 dark pixels in the region after an `Ok`. No producer is known to write one, so
            // it is refused rather than modelled (DECISIONS.md rule 1).
            if entry.stream_dict().key(&MATRIX).type_code() != object_type::NULL {
                return Err(Error::Unsupported(
                    "pdf redaction [type-three-procedure-matrix]: a Type 3 glyph procedure carrying \
                     a /Matrix of its own, which moves what it draws in a way burrow does not model"
                        .to_owned(),
                ));
            }
            // A STREAM IS ALWAYS INDIRECT, so its identity is never the direct-object `(0, 0)`
            // that would make two different procedures look like one.
            let identity = pack(entry.object()?);
            let image = if let Some(&cached) = scans.procedures.get(&identity) {
                cached
            } else {
                let Some(procedure) = entry.stream_data()? else {
                    // Undecodable, so what it draws is unknown, and unknown is not "no text".
                    return Err(Error::Malformed(
                        "pdf redaction [type-three-unreadable]: a Type 3 glyph procedure whose \
                         data burrow could not decode"
                            .to_owned(),
                    ));
                };
                let draws = check_type_three_procedure(&procedure, watch)?;
                scans.procedures.insert(identity, draws.image);
                draws.image
            };
            extents.extend(image);
        }
        if let Some(id) = procs_identity {
            scans.char_procs.insert(id, extents.clone());
        }
        extents
    };
    // PER FONT, EVERY TIME: the box is this font's, whoever scanned the procedures.
    for extent in &extents {
        watch.tick()?;
        check_type_three_image_inside(*extent, font_bbox)?;
    }
    Ok(!extents.is_empty())
}

/// Every `/Font` resource name on the page.
fn font_names<O: PdfObject>(resources: &PageResources<O>) -> Result<Vec<Name>> {
    let fonts = resources.dictionary().key(&FONT);
    if fonts.type_code() != object_type::DICTIONARY {
        return Ok(Vec::new());
    }
    crate::pdfsyntax::dict::top_level_keys(&fonts.unparse())?
        .iter()
        .map(|key| Name::from_stripped(key))
        .collect()
}

/// The form with this identity, by content.
fn find_form<O: PdfObject>(resources: &PageResources<O>, id: u64) -> Result<Vec<u8>> {
    form_handle(resources, id)?.stream_data()?.ok_or_else(|| {
        Error::Malformed(
            "pdf redaction [form-unreadable]: a Form XObject whose data burrow could not \
                 decode"
                .to_owned(),
        )
    })
}

/// The streams the marked-content rule reads, and what it needs to read each one.
struct MarkedContentScope {
    /// The page's `Do` names that lead to a removed glyph.
    page_names: BTreeSet<Vec<u8>>,
    /// What the page stream's `BDC` names resolve to.
    page_properties: NamedProperties,
    /// Each in-scope form: its identity, its own leading `Do` names, and what its `BDC` names
    /// resolve to.
    forms: Vec<(u64, BTreeSet<Vec<u8>>, NamedProperties)>,
}

/// Which scope a stream's names resolve against: the page's resources, or a form's own.
///
/// `None` is the page. A form that declares no `/Resources` has no scope of its own and takes
/// its enclosure's, which is why this is recorded per path rather than per form.
type ScopeOwner = Option<u64>;

/// The `/Properties` of every scope read so far, and which scopes each form resolves through.
struct PropertyScopes<'d> {
    /// Each scope's `/Properties`, read once however many forms resolve through it.
    read: BTreeMap<ScopeOwner, NamedProperties>,
    /// The scopes each in-scope form's names resolve against — more than one when a form
    /// declaring no `/Resources` is reached from two enclosures.
    owners: BTreeMap<u64, BTreeSet<ScopeOwner>>,
    /// Property lists left to read, against [`MAX_PROPERTY_LISTS`].
    budget: usize,
    /// Each indirect property list already read, by identity. Four thousand names may point at
    /// one object; it is unparsed and classified once.
    classified: BTreeMap<(c_int, c_int), PropertyList>,
    deadline: &'d Deadline,
    clock: &'d Arc<dyn Clock>,
}

/// How many `/Properties` entries one redaction will resolve, across every scope it reads.
///
/// Each is an `unparse` through the engine. One dictionary is already capped, by
/// `pdfsyntax::dict::MAX_KEYS`, so what this bounds is the **total across scopes**: every form on
/// a path to a removed glyph brings its own `/Properties`, and without a total the work is forms
/// times keys. A tagged or layered page carries a handful; four thousand is past any producer and
/// small enough that the work is bounded well inside the deadline.
const MAX_PROPERTY_LISTS: usize = 4096;

impl PropertyScopes<'_> {
    /// Read `dictionary`'s `/Properties` as the scope `owner`, unless it has been read already.
    ///
    /// Only **dictionary** entries are recorded. Anything else — a stream, a number, a missing
    /// object — resolves to nothing, and nothing refuses as unresolved: the cautious reading of
    /// an entry this cannot interpret.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] naming `properties-too-many` past [`MAX_PROPERTY_LISTS`], and
    /// whatever reading a key failed with.
    fn read<O: PdfObject>(&mut self, owner: ScopeOwner, dictionary: &O) -> Result<()> {
        const PROPERTIES: Name = Name::literal(b"/Properties\0");
        if self.read.contains_key(&owner) {
            return Ok(());
        }
        let mut resolved = NamedProperties::default();
        let properties = dictionary.key(&PROPERTIES);
        if properties.type_code() == object_type::DICTIONARY {
            for key in crate::pdfsyntax::dict::top_level_keys(&properties.unparse())? {
                // A REFUSAL, NOT A TRUNCATION. Stopping early would leave the rest of the names
                // unresolved, which refuses them anyway -- but under a rule that says the file
                // could not be read, when what happened is that burrow stopped reading.
                self.budget = self.budget.checked_sub(1).ok_or_else(|| {
                    Error::Unsupported(
                        "pdf redaction [properties-too-many]: a page whose marked-content \
                         property lists are more than burrow will resolve"
                            .to_owned(),
                    )
                })?;
                // A CHECKPOINT PER ENTRY. Each is an engine call and a lex, and the entries are
                // the file's number.
                self.deadline.checkpoint(self.clock.as_ref())?;
                let entry = properties.key(&Name::from_stripped(&key)?);
                if entry.type_code() != object_type::DICTIONARY {
                    continue;
                }
                // ONCE PER OBJECT. `unparse` resolves the entry itself and leaves any reference
                // INSIDE it as `N G R`, which `PropertyList::read` treats as a door it did not
                // open. A direct entry has no identity (`(0, 0)`) and exists in one place only.
                let identity = entry.object()?;
                let list = match self.classified.get(&identity) {
                    Some(list) if identity != (0, 0) => *list,
                    _ => {
                        let list = PropertyList::read(&entry.unparse());
                        if identity != (0, 0) {
                            self.classified.insert(identity, list);
                        }
                        list
                    }
                };
                // `top_level_keys` returns decoded names without the slash, which is how
                // `Operand::Name` carries them too, so the two compare as they are.
                resolved.insert(key, list);
            }
        }
        self.read.insert(owner, resolved);
        Ok(())
    }

    /// What a stream resolving through `owners` may name: the union of every scope's lists.
    fn union(&self, owners: impl IntoIterator<Item = ScopeOwner>) -> NamedProperties {
        let mut union = NamedProperties::default();
        for owner in owners {
            if let Some(read) = self.read.get(&owner) {
                union.extend(read);
            }
        }
        union
    }
}

/// Every stream the marked-content rule must read, and the `Do` names in each that lead to a
/// removed glyph.
///
/// Returns the page's own names, then one entry per **form** that holds or draws toward a
/// removed glyph, with that form's own leading names.
///
/// # Why "draws toward", and what asking a narrower question cost
///
/// [`check_marked_content`] was called on the page stream and then once per form **holding** a
/// cut glyph. A form that merely *draws* such a form was never in that set, so its bytes were
/// never read — and a `/Span << /ActualText … >> BDC /Inner Do … EMC` sitting in that
/// intermediate form was seen by nobody. Measured by a security review on a 1,192-byte file:
/// the operation returned `Ok`, the glyphs came out of `Inner`, and PDFium read
/// `BURROW-SEC164-MIDFORM` off the output.
///
/// So the scope is every form on a path from the page to a removed glyph, not the forms at the
/// end of one.
///
/// # And a form declaring no `/Resources` inherits, which the descent has to do too
///
/// `Resources::within` returns `None` for a form with no `/Resources`, and the walk then
/// resolves that form's names against the **enclosing** dictionary — so its children are the
/// enclosing scope's children. This descent returned "reaches nothing" instead, which unlinked
/// the page-level `Do` from the wanted set: deleting one dictionary from a file turned a
/// correct refusal into a leak, also measured. Anything that resolves names differently from
/// the walk is a bypass by construction.
///
/// # Errors
///
/// [`Error::Unsupported`] when the visit budget or the depth ceiling is reached.
fn marked_content_scope<O: PdfObject>(
    resources: &PageResources<O>,
    wanted: &BTreeSet<u64>,
    deadline: &Deadline,
    clock: &Arc<dyn Clock>,
) -> Result<MarkedContentScope> {
    let mut found: BTreeMap<u64, BTreeSet<Vec<u8>>> = BTreeMap::new();
    // THE PAGE'S OWN PROPERTIES WHETHER OR NOT ANY FORM IS WANTED. A glyph the page draws
    // itself can sit inside `/P /MC0 BDC`, and a page with no forms at all is the commonest
    // case there is.
    let mut scopes = PropertyScopes {
        read: BTreeMap::new(),
        owners: BTreeMap::new(),
        budget: MAX_PROPERTY_LISTS,
        classified: BTreeMap::new(),
        deadline,
        clock,
    };
    scopes.read(None, resources.dictionary())?;
    let page_properties = scopes.union([None]);
    if wanted.is_empty() {
        return Ok(MarkedContentScope {
            page_names: BTreeSet::new(),
            page_properties,
            forms: Vec::new(),
        });
    }
    // ONE BUDGET ACROSS THE WHOLE SCOPE, not one per entry: a per-entry budget would let each of
    // `n` top-level forms pay the full ceiling, which is `n` times the ceiling.
    let mut budget = MAX_FORM_RESOURCE_VISITS;
    let mut open = BTreeSet::new();
    let page_names = scope_of(
        resources.dictionary(),
        None,
        0,
        wanted,
        &mut Visit {
            budget: &mut budget,
            open: &mut open,
            found: &mut found,
            scopes: &mut scopes,
        },
    )?;
    let forms = found
        .into_iter()
        .map(|(form, names)| {
            let owners = scopes.owners.get(&form).cloned().unwrap_or_default();
            (form, names, scopes.union(owners))
        })
        .collect();
    Ok(MarkedContentScope {
        page_names,
        page_properties,
        forms,
    })
}

/// The state one scope walk threads through its recursion.
struct Visit<'s, 'd> {
    budget: &'s mut usize,
    open: &'s mut BTreeSet<u64>,
    found: &'s mut BTreeMap<u64, BTreeSet<Vec<u8>>>,
    scopes: &'s mut PropertyScopes<'d>,
}

/// Spend one visit from the shared budget, refusing when it runs out.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `form-graph-too-large`. **Exhaustion is a refusal, not a
/// silent "nothing found"**: a "nothing found" here would mean "this `Do` draws nothing the
/// removal reaches", which is the answer that re-opens the `/ActualText` cross-stream hole.
fn spend(budget: &mut usize) -> Result<()> {
    *budget = budget.saturating_sub(1);
    if *budget == 0 {
        return Err(Error::Unsupported(
            "pdf redaction [form-graph-too-large]: a page whose Form XObjects reference each \
             other more ways than burrow will walk"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The names in `dict`'s `/XObject` that lead to a form in `wanted`, recording each form that
/// does in `found` — and the scope its own names resolve against, `owner` being `dict`'s.
fn scope_of<O: PdfObject>(
    dict: &O,
    owner: ScopeOwner,
    depth: usize,
    wanted: &BTreeSet<u64>,
    visit: &mut Visit<'_, '_>,
) -> Result<BTreeSet<Vec<u8>>> {
    const XOBJECT: Name = Name::literal(b"/XObject\0");
    const RESOURCES: Name = Name::literal(b"/Resources\0");
    let mut names = BTreeSet::new();
    // A REFUSAL, NOT AN EMPTY SET. Returning `names` here says "this subtree draws nothing the
    // removal reaches", and every leak on this branch has been some version of an answer
    // smaller than the truth: the scope that stopped at the forms holding a glyph, the descent
    // that stopped at a form with no `/Resources`, the memo that kept the first path's answer.
    // A silent truncation at depth 16 is the same shape waiting to happen.
    //
    // IT IS REACHABLE, which I got wrong at first: the draft said it could not fire because
    // `glyphs_in` refuses a document nested past `MAX_FORM_DEPTH` before a glyph that deep can
    // become a cut glyph. True, and irrelevant -- this walk descends the resource graph
    // including **undrawn** subtrees the geometry walk never enters, and a bomb fixture reached
    // it immediately.
    //
    // # No test fails if this goes back to truncating, and that is worth saying plainly
    //
    // A mutation replacing this with `Ok(names)` survives the suite. It is not an oversight in
    // the fixtures: for truncation to *leak*, the discarded subtree would have to contain a
    // form holding a removed glyph, and such a form was reached by `glyphs_in` at depth 15 or
    // less -- so the scope walk reaches it too, by a path no longer than the one the geometry
    // walk took. I could not construct a document where truncating here loses a wanted form.
    //
    // It refuses anyway, for two reasons. The argument above is a **cross-module** one, resting
    // on a cap in `glyphs_in` staying in step with this one; that exact kind of reasoning has
    // been wrong three times on this branch, each time as a leak. And the cost is a refusal on
    // a legal-but-absurd shape -- an undrawn sixteen-deep form chain -- which is the direction
    // this operation is supposed to fail in. `CLAUDE.md` says an unreachable guard reads as
    // coverage and is not any; this one is reachable, untestable-for-harm, and says so.
    if depth >= crate::pdfsyntax::geometry::MAX_FORM_DEPTH {
        return Err(Error::Unsupported(
            "pdf redaction [form-graph-too-deep]: a page whose Form XObjects nest deeper than \
             burrow will walk"
                .to_owned(),
        ));
    }
    // A `/Resources` WITH NO `/XObject` IS ORDINARY, and asking for its keys is not. The flat
    // version only ever saw the page's resources, which always have one; descending reaches
    // forms whose resources carry only a `/Font`, and `top_level_keys` on a non-dictionary is
    // `Malformed` -- a refusal blaming the file for a question burrow should not have asked.
    let xobjects = dict.key(&XOBJECT);
    if xobjects.type_code() != object_type::DICTIONARY {
        return Ok(names);
    }
    for key in crate::pdfsyntax::dict::top_level_keys(&xobjects.unparse())? {
        let entry = xobjects.key(&Name::from_stripped(&key)?);
        if entry.type_code() != object_type::STREAM {
            continue;
        }
        spend(visit.budget)?;
        let here = pack(entry.object()?);
        if !visit.open.insert(here) {
            continue;
        }
        let own = entry.stream_dict().key(&RESOURCES);
        // INHERITING WHEN IT DECLARES NONE, exactly as `Resources::within` does. Recursing with
        // the enclosing dictionary re-examines its entries, and `open` is what stops that being
        // endless: every form on the current path is already in it.
        //
        // AND ITS `BDC` NAMES RESOLVE THE SAME WAY, which is the scope rule #166 exists for: a
        // form with its own `/Resources` resolves `/MC0` there and nowhere else, and a form with
        // none resolves it wherever its enclosure does. A page-level answer for a form's span is
        // the font defect again -- a decoy on the page shadowing the list that is actually read.
        let declares_own = own.type_code() == object_type::DICTIONARY;
        let resolves_through: ScopeOwner = if declares_own { Some(here) } else { owner };
        let inner = if declares_own {
            scope_of(&own, Some(here), depth.saturating_add(1), wanted, visit)?
        } else {
            scope_of(dict, owner, depth.saturating_add(1), wanted, visit)?
        };
        visit.open.remove(&here);
        if wanted.contains(&here) || !inner.is_empty() {
            // THE SCOPE THIS FORM'S OWN NAMES RESOLVE THROUGH ON THIS PATH, read once per scope
            // and recorded per path -- the union, for the reason `found` below is a union.
            visit
                .scopes
                .read(resolves_through, if declares_own { &own } else { dict })?;
            visit
                .scopes
                .owners
                .entry(here)
                .or_default()
                .insert(resolves_through);
            // WITHOUT THE LEADING SLASH, which is how `Operand::Name` carries a decoded name.
            names.insert(key.strip_prefix(b"/".as_slice()).unwrap_or(&key).to_vec());
            // THE UNION OVER PATHS, not the first path's answer.
            //
            // `inner` is NOT a property of the form. A form declaring no `/Resources` resolves
            // its children against whatever encloses it, so the same form reached by two routes
            // yields two different name sets — and `or_insert` kept whichever route the walk
            // happened to take first. When that was the smaller set, `check_marked_content` was
            // told a `Do` in that form's stream draws nothing the removal reaches, and the
            // `/ActualText` span around it was never examined.
            //
            // Measured by a code review on a 1,438-byte file: a form reached both from the page
            // (where it inherits the page's names) and from an intermediate form (where it
            // resolves to the two forms holding the glyphs). The page route ran first, stored a
            // non-empty but wrong set, and the operation returned `Ok` with the carrier in the
            // output. The single-parent control refused correctly.
            //
            // It is the third instance of one sentence: **anything that resolves names
            // differently from the walk is a bypass by construction** — here the divergence is
            // in the memo rather than the resolver. Union is the conservative direction: more
            // names means more checking, never less.
            visit.found.entry(here).or_default().extend(inner);
        }
    }
    Ok(names)
}

/// How many resource-graph entries one lookup will examine, across the whole descent.
///
/// # Depth is not the bound it looks like, and this crate had already measured that
///
/// [`crate::pdfsyntax::geometry::MAX_FORM_DRAWS`]' rustdoc says it, about the walk:
/// `MAX_FORM_DEPTH` plus a set of forms currently open *look* like a bound and are not, because
/// a form may name the next one `B` times without ever recursing into itself. The open set is a
/// **path** set — an entry is removed on the way out so a legitimate second path is not skipped
/// — so a resource graph that is a DAG is re-explored once per path, and the work is `B^16`.
///
/// I reintroduced exactly that, in the lookup, two commits after reading the paragraph warning
/// about it. Measured on this tree, release, through `burrow_ops::redact::page`, with the bomb
/// never drawn so the walk's own `MAX_FORM_DRAWS` never sees it:
///
/// | levels | branch | file | elapsed |
/// |---|---|--:|--:|
/// | 4 | 3 | 1,965 B | 3 ms |
/// | 8 | 3 | 2,697 B | 45 ms |
/// | 12 | 3 | 3,429 B | **2.77 s** |
///
/// Nine times per two levels, which is `3²`. A 4 KB file at depth 16 branch 4 does not return.
///
/// So the ceiling is a **total**, counted across the whole descent and — in
/// [`marked_content_scope`] — across its whole descent, because a per-entry budget would let
/// each of `n` entries pay the full price. Four thousand and ninety-six, the same number and
/// the same argument as `MAX_FORM_DRAWS`.
const MAX_FORM_RESOURCE_VISITS: usize = 4096;

/// Walk a `/Resources` dictionary's `/XObject` entries, and each form's own, looking for `want`.
///
/// # Two loops again, and the comment that used to deny it
///
/// This said "one function rather than two near-copies", written when `find_form_below` and
/// `reaches_wanted` were merged into it — *a ceiling that must be written twice is a ceiling
/// that gets written once*. [`scope_of`] then arrived and is the same loop a third time, so the
/// claim was false of the file it was written in, and a code review said so.
///
/// They stay separate because they answer different questions — this one wants **a handle**,
/// `scope_of` wants **the names a stream spells** — and the differences are real rather than
/// incidental: only `scope_of` inherits, only `scope_of` records. What is shared is the ceiling,
/// and that **is** in one place now ([`spend`]), which was the original point.
///
/// The honest resolution is smaller than either: `scope_of` already visits every form on every
/// path and could carry each one's handle, which would make this function unnecessary. Filed
/// rather than done, because it changes `form_handle`'s callers.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `form-graph-too-large` when the budget is exhausted, and
/// whatever reading a dictionary failed with. **Exhaustion is a refusal, not a `None`**: for
/// [`scope_of`] a `None` would mean "this `Do` draws nothing the removal reaches", which is
/// the answer that re-opens the `/ActualText` cross-stream hole. The conservative direction and
/// the honest one are the same here.
fn walk_forms<O: PdfObject>(
    resources: &O,
    depth: usize,
    budget: &mut usize,
    want: &mut dyn FnMut(u64) -> bool,
    open: &mut BTreeSet<u64>,
) -> Result<Option<O>> {
    const XOBJECT: Name = Name::literal(b"/XObject\0");
    const RESOURCES: Name = Name::literal(b"/Resources\0");
    if depth >= crate::pdfsyntax::geometry::MAX_FORM_DEPTH {
        return Ok(None);
    }
    let xobjects = resources.key(&XOBJECT);
    if xobjects.type_code() != object_type::DICTIONARY {
        return Ok(None);
    }
    for key in crate::pdfsyntax::dict::top_level_keys(&xobjects.unparse())? {
        let entry = xobjects.key(&Name::from_stripped(&key)?);
        if entry.type_code() != object_type::STREAM {
            continue;
        }
        // COUNTED PER ENTRY EXAMINED, which is the quantity that grew exponentially -- not per
        // level and not per form, both of which stayed small while this ran for seconds.
        spend(budget)?;
        let here = pack(entry.object()?);
        if want(here) {
            return Ok(Some(entry));
        }
        // A CYCLE IS A DOCUMENT THAT EXISTS, not a malformed one, and `open` is what makes this
        // terminate. It is a path set: the entry comes off on the way out, so a form legitimately
        // reachable by two routes is still found by the second.
        if !open.insert(here) {
            continue;
        }
        // NO INHERITANCE FALLBACK HERE, and that is a difference from `scope_of` rather than an
        // omission. This answers "is the form with this identity reachable", and a form
        // declaring no `/Resources` has its children named in the **enclosing** dictionary --
        // which this loop is already iterating. Descending into it again could only re-find
        // entries already covered, which is why a code review's mutation removing the fallback
        // changed no test: it was dead weight.
        //
        // `scope_of` genuinely needs it, because it wants the name as *that stream* spells it,
        // and only the enclosing dictionary has it.
        let own = entry.stream_dict().key(&RESOURCES);
        if own.type_code() == object_type::DICTIONARY
            && let Some(found) = walk_forms(&own, depth.saturating_add(1), budget, want, open)?
        {
            return Ok(Some(found));
        }
        open.remove(&here);
    }
    Ok(None)
}

/// The handle for the form with this identity, searched the way the walk reached it.
///
/// # It searched the page's `/XObject` only, and the walk does not stop there
///
/// The geometry walk descends into a form's **own** `/Resources` — that is what
/// `Resources::within` exists for — so a glyph can legitimately carry
/// `source.form: Some(id)` for a form the page's `/XObject` never names. This looked in the page
/// only, failed to find such a form, and returned `Error::Malformed` saying the form "is not in
/// the page's resources": true, irrelevant, and phrased as though the document were at fault.
///
/// Measured (#164): `evade-oc-two-levels-down.pdf` and `nearmiss-nested-forms-no-oc.pdf` were
/// both refused by `form-vanished`. The second is a **near-miss twin** whose entire purpose is
/// that it has no optional content and must therefore redact — so the corpus was reporting a
/// refusal where its own fixture design says there should be none.
///
/// Nothing leaked: it refuses. What it did was blame the file for an incomplete lookup, and take
/// a document offline that the operation can handle.
///
/// # The ceilings, and the one that was missing
///
/// `MAX_FORM_DEPTH` bounds the descent and the set of forms already open stops a cycle. This
/// said those two were "the same ceilings as the walk"; they are not, and the sentence was
/// wrong when it was written. `glyphs_in` carries a **third** — a total count of forms drawn,
/// against `MAX_FORM_DRAWS` — precisely because depth and a path set do not bound a DAG.
/// [`MAX_FORM_RESOURCE_VISITS`] is this lookup's equivalent, and it is why the descent now
/// terminates on a graph a code review measured running for seconds from a 3 KB file.
fn form_handle<O: PdfObject>(resources: &PageResources<O>, id: u64) -> Result<O> {
    let mut open = BTreeSet::new();
    let mut budget = MAX_FORM_RESOURCE_VISITS;
    walk_forms(
        resources.dictionary(),
        0,
        &mut budget,
        &mut |here| here == id,
        &mut open,
    )?
    .ok_or_else(|| {
        Error::Malformed(
            "pdf redaction [form-vanished]: a form the walk found is not reachable from the \
             page's resources"
                .to_owned(),
        )
    })
}

/// Remove a font's entries for codes the document no longer draws.
///
/// **Narrowed, not deleted.** Both of the first two edits here started as a removal of the
/// whole structure, and both were wrong in the same way: `/ToUnicode` and `/Differences` say
/// what the *kept* codes are as well as what the removed ones were, so deleting them damages
/// the text the redaction was meant to leave alone. `/ToUnicode` was measured — PDFium read
/// `U+0001` at the origin where readable text had been. `/Differences` was not measured; it is
/// the same shape, found by looking again at the other half of the same function.
fn narrow_font<O: PdfObject>(font: &O, keeps: &BTreeSet<u32>) -> Result<()> {
    #[cfg(test)]
    let skip = super::hooks::narrowing_skipped();
    #[cfg(not(test))]
    let skip = false;
    if !skip {
        narrow_to_unicode(font, keeps)?;
        narrow_differences(font, keeps)?;
    }

    // `/Widths` is positional, so an entry cannot be removed without moving every later code.
    // Zeroing the removed ones keeps the array's shape and says nothing about what was there.
    let widths = font.key(&WIDTHS);
    if widths.type_code() == object_type::ARRAY {
        let first = first_char(font)?;
        let length = widths.array_len();
        if length > DIFFERENCES_CEILING {
            return Err(Error::Unsupported(
                "pdf redaction [widths-too-long]: a /Widths array longer than burrow will \
                 rewrite"
                    .to_owned(),
            ));
        }
        for at in 0..length {
            let Ok(offset) = u32::try_from(at) else {
                continue;
            };
            let code = first.saturating_add(offset);
            if !keeps.contains(&code) {
                widths.set_array_item(at, &widths.integer_beside(0))?;
            }
        }
    }
    // DRAINED, and it was not. The `/Widths` loop ran after both narrowings had drained and
    // then returned `Ok(())` with nothing of its own — so a failure writing a width surfaced
    // at the NEXT `take_error`: a different font's `/ToUnicode`, or the page strip, or the
    // write. A wrong answer attributed to the wrong cause, which is the failure mode
    // `prune.rs`'s drain-after-every-call rule exists for. THROUGH THE FONT, the handle this
    // function was given, so it is the font's document that is drained and no other (#191).
    font.drained()
}

/// Rewrite `/ToUnicode` so it maps the kept codes and no others.
///
/// `/ToUnicode` IS THE REMOVED CHARACTER, in plain text, beside the page it was cut from, so
/// leaving it is not an option. Deleting it is not one either: see this function's caller.
/// `qpdf_oh_replace_stream_data` is trapped (ADR 0013), so the narrowed program can be written
/// back, and the filter arguments are nulls — the stream is stored uncompressed, which the
/// write path's `ObjectStreams::Preserve` does not undo. A `/ToUnicode` is a few kilobytes.
fn narrow_to_unicode<O: PdfObject>(font: &O, keeps: &BTreeSet<u32>) -> Result<()> {
    let to_unicode = font.key(&TO_UNICODE);
    if to_unicode.type_code() != object_type::STREAM {
        return Ok(());
    }
    let Some(program) = to_unicode.stream_data()? else {
        // Undecodable: burrow cannot say what it maps, so it cannot say it is safe to keep.
        // Removing it loses the kept codes' text, which is a legibility cost rather than a
        // leak, and is the only honest answer available here.
        font.remove_key(&TO_UNICODE);
        return font.drained();
    };
    let read = ToUnicode::parse(&program)?;
    match read.narrowed(&|code| keeps.contains(&code)) {
        Some(narrowed) => {
            let null = to_unicode.null_beside();
            to_unicode.replace_stream_data(&narrowed, &null, &null)?;
        }
        // Nothing the CMap mapped is still drawn, so there is no text to preserve and a CMap
        // with no `bfchar` section is not a CMap.
        None => font.remove_key(&TO_UNICODE),
    }
    font.drained()
}

/// Rewrite `/Differences` so it names the kept codes and no others.
///
/// # Why an integer, and not a removal
///
/// `/Differences` is `[ 65 /S /e /c /r /e /t ]`: a code, then the glyph names for that code
/// onwards. The names spell the text. But they spell the *kept* text too, so dropping the array
/// re-points every kept code at the base encoding's glyph — different characters drawn, which
/// is corruption rather than redaction.
///
/// Erasing one name is not available either: [`PdfObject::erase_item`] renumbers, so every
/// later name would slide onto the wrong code. And building a replacement array is closed —
/// `qpdf_oh_new_array` and `qpdf_oh_new_name` are untrapped and do not meet
/// `engines/qpdf-untrapped-accepted.toml`'s non-parsing bar.
///
/// What is available is [`PdfObject::set_array_item`] with an integer, and it happens to be
/// exactly right. Replacing the name at index `i`, whose code is `c`, with the integer `c + 1`
/// leaves the array the same length and re-anchors the run at the code the next item already
/// had. `[1 /a /b /c]` with `/b` removed becomes `[1 /a 3 /c]`, and `/c` is still code 3.
fn narrow_differences<O: PdfObject>(font: &O, keeps: &BTreeSet<u32>) -> Result<()> {
    let encoding = font.key(&ENCODING);
    if encoding.type_code() != object_type::DICTIONARY {
        return Ok(());
    }
    let differences = encoding.key(&DIFFERENCES);
    if differences.type_code() != object_type::ARRAY {
        return Ok(());
    }
    let length = differences.array_len();
    if length > DIFFERENCES_CEILING {
        return Err(Error::Unsupported(
            "pdf redaction [differences-too-long]: a /Differences array longer than burrow              will rewrite"
                .to_owned(),
        ));
    }

    // TWO PASSES, BECAUSE SEVERAL NAMES CAN SIT AT ONE CODE AND ONLY THE LAST ONE DRAWS.
    //
    // `[0 /S 0 /e 0 /c 0 /r]` is legal PDF: four assignments to code 0, of which a reader takes
    // the last. A single pass that asked "is this code kept?" kept every one of them, so a
    // document whose kept code happens to be 0 carried the removed text's spelling out intact
    // — measured, with `Ok` and `cut: true` beside it.
    //
    // So the first pass finds, for each code, the index of the last name assigned to it. The
    // second keeps a name only if its code is kept **and** it is that index.
    let mut last_at: BTreeMap<u32, c_int> = BTreeMap::new();
    walk_differences(&differences, length, |code, at| {
        last_at.insert(code, at);
    })?;

    let mut code: u32 = 0;
    for at in 0..length {
        let item = differences.array_item(at);
        if item.type_code() == object_type::INTEGER {
            code = differences_anchor(&item)?;
            continue;
        }
        if item.type_code() != object_type::NAME {
            // `walk_differences` above refused this already; kept so the two loops cannot drift.
            return Err(differences_item_unreadable());
        }
        let survives = keeps.contains(&code) && last_at.get(&code) == Some(&at);
        if !survives {
            let next = i64::from(code).saturating_add(1);
            differences.set_array_item(at, &differences.integer_beside(next))?;
        }
        code = code.saturating_add(1);
    }
    font.drained()
}

/// Walk a `/Differences` array, calling `seen` with each name's code and index.
fn walk_differences<O: PdfObject>(
    differences: &O,
    length: c_int,
    mut seen: impl FnMut(u32, c_int),
) -> Result<()> {
    let mut code: u32 = 0;
    for at in 0..length {
        let item = differences.array_item(at);
        if item.type_code() == object_type::INTEGER {
            code = differences_anchor(&item)?;
            continue;
        }
        if item.type_code() != object_type::NAME {
            return Err(differences_item_unreadable());
        }
        seen(code, at);
        code = code.saturating_add(1);
    }
    Ok(())
}

/// The refusal for a `/Differences` item that is neither an integer anchor nor a name (#125's
/// third security review).
///
/// These loops SKIPPED one. PDFium reads any non-name item as an anchor -- its integer value, so
/// a string, a real, a boolean -- which moves every name after it to another code. Measured with
/// `[65 (x) /S /E /C /R /E /T]`: PDFium extracted SECRET from codes 0 to 5, burrow narrowed the
/// names as codes 65 to 70, and the output kept `/S /E /C /R /E /T` with `Ok` and `cut: true`
/// beside it. The same with `0.5` or `true` in place of `(x)`. Refused, not modelled
/// (DECISIONS.md rule 1).
pub(super) fn differences_item_unreadable() -> Error {
    Error::Unsupported(
        "pdf redaction [differences-item-unreadable]: a /Differences item that is neither a \
         code nor a glyph name, which a renderer reads as a code and burrow would not"
            .to_owned(),
    )
}

/// A `/Differences` anchor, refused rather than folded onto zero.
///
/// It was `u32::try_from(value).unwrap_or(0)`, which is the silent truncation `core/CLAUDE.md`
/// denies `as` casts for, spelled differently. An anchor of `-1` or `2^32` became code 0, so
/// every name after it was tested against the wrong code — and a glyph name spelling removed
/// text stayed in the file because its mis-computed code happened to be kept. Measured: a
/// `/Differences` of `[-1 /S /e /c /r]` came back unchanged.
fn differences_anchor<O: PdfObject>(item: &O) -> Result<u32> {
    u32::try_from(item.integer_value()).map_err(|_| {
        Error::Malformed(
            "pdf redaction [differences-anchor]: a /Differences array anchored at a code              outside the range a character code can take"
                .to_owned(),
        )
    })
}

/// A font's `/FirstChar`, read the way the resolver reads a number.
///
/// # One key, two readings, and they disagreed
///
/// This required `object_type::INTEGER` while `resources::whole` accepts a real that is a whole
/// number. Measured with `/FirstChar 65.0`: the resolver placed the glyphs from code 65 and
/// this read the first char as 0, so the `/Widths` offsets were 65 entries out and the KEPT
/// glyph's width was zeroed along with the removed ones. The direction is corruption rather
/// than leakage, and it is still two readings of one key that are supposed to agree.
fn first_char<O: PdfObject>(font: &O) -> Result<u32> {
    const FIRST_CHAR: Name = Name::literal(b"/FirstChar\0");
    let value = font.key(&FIRST_CHAR);
    let code = match value.type_code() {
        object_type::INTEGER => u32::try_from(value.integer_value()).ok(),
        object_type::REAL => crate::pdfsyntax::ops::numbers_in(&value.unparse())
            .first()
            .copied()
            .and_then(super::resources::whole)
            .and_then(|whole| u32::try_from(whole).ok()),
        // Absent. `/FirstChar` is required beside `/Widths`, but a font with neither is
        // handled by the `ARRAY` guard above rather than here.
        _ => Some(0),
    };
    code.ok_or_else(|| {
        Error::Malformed(
            "pdf redaction [first-char]: a font whose /FirstChar is not a character code"
                .to_owned(),
        )
    })
}

#[cfg(all(test, feature = "native-engines", burrow_native_engines))]
mod annotation_walk_tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use burrow_types::{Clock, Deadline, Limits};

    use super::remove_annotations_in;
    use crate::pdfsyntax::geometry::Rect;
    use crate::redact::graph::{OpensForRedaction, PdfDocument};

    /// A clock that counts how often it is read and moves a millisecond each time.
    struct Counting(AtomicU64);
    impl Clock for Counting {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[test]
    fn the_annotation_walk_itself_reads_the_deadline_per_annotation_and_stream() {
        // THE WALK'S OWN READS, which no end-to-end test can see: the sharing walk runs first and
        // reads the deadline per annotation too, so a deadline passing mid-document stops there.
        // Called directly here, over 40 kept annotations each with its own appearance stream.
        let annotations = 40;
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [{}] >>",
                (0..annotations)
                    .map(|i| format!("{} 0 R ", 4 + 2 * i))
                    .collect::<String>()
            ),
        ];
        for i in 0..annotations {
            objects.push(format!(
                "<< /Type /Annot /Subtype /Stamp /Rect [20 50 120 70] /AP << /N {} 0 R >> >>",
                5 + 2 * i
            ));
            objects.push(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Length 0 >>\nstream\n\nendstream"
                    .to_owned(),
            );
        }
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        let options = crate::OpenOptions::new(
            Limits::default(),
            std::sync::Arc::new(burrow_types::ManualClock::new(0)) as std::sync::Arc<dyn Clock>,
        );
        let (document, _) = crate::qpdf::Qpdf
            .open_for_redaction(out.as_bytes(), &options)
            .expect("opens");
        let page = document.page(0).expect("page 0");
        let clock = Counting(AtomicU64::new(0));
        let deadline = Deadline::start(&clock, &Limits::default());
        let before = clock.0.load(Ordering::Relaxed);
        let region = Rect {
            left: 0.0,
            bottom: 330.0,
            right: 300.0,
            top: 370.0,
        };
        remove_annotations_in(&page, &region, 0, &deadline, &clock).expect("all kept, bounded");
        let reads = clock.0.load(Ordering::Relaxed) - before;
        // THREE PER KEPT ANNOTATION since #239: the read of it, the decision on it, and its stream.
        assert!(
            reads >= 3 * annotations,
            "{reads} deadline reads over {annotations} kept annotations and as many streams: the \
             walk must read it at each"
        );
    }

    #[test]
    fn the_annotation_walk_reads_the_deadline_through_every_dependent() {
        // #239's PASSES: a reply chain of 40, the first over the region, so all 40 go -- each read,
        // reached by the closure and erased, and the deadline read at each of the three.
        let annotations: u64 = 40;
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [{}] >>",
                (0..annotations)
                    .map(|i| format!("{} 0 R ", 4 + i))
                    .collect::<String>()
            ),
            "<< /Type /Annot /Subtype /Text /Rect [20 340 120 360] /Contents (ROOT) >>".to_owned(),
        ];
        for i in 1..annotations {
            objects.push(format!(
                "<< /Type /Annot /Subtype /Text /Rect [200 50 280 90] /IRT {} 0 R >>",
                3 + i
            ));
        }
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        let options = crate::OpenOptions::new(
            Limits::default(),
            std::sync::Arc::new(burrow_types::ManualClock::new(0)) as std::sync::Arc<dyn Clock>,
        );
        let (document, _) = crate::qpdf::Qpdf
            .open_for_redaction(out.as_bytes(), &options)
            .expect("opens");
        let page = document.page(0).expect("page 0");
        let clock = Counting(AtomicU64::new(0));
        let deadline = Deadline::start(&clock, &Limits::default());
        let before = clock.0.load(Ordering::Relaxed);
        let region = Rect {
            left: 0.0,
            bottom: 330.0,
            right: 300.0,
            top: 370.0,
        };
        let removed =
            remove_annotations_in(&page, &region, 0, &deadline, &clock).expect("all removed");
        assert_eq!(
            removed.annotations.len(),
            40,
            "the whole chain goes with its root"
        );
        let reads = clock.0.load(Ordering::Relaxed) - before;
        assert!(
            reads >= 3 * annotations,
            "{reads} deadline reads over {annotations} removed annotations: the walk must read it \
             at each in every pass"
        );
    }
}
