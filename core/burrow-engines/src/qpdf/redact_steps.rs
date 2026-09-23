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
//! `&mut Document` would leave the caller able to write out a half-edited one. This takes the
//! `Document` by value and the only path to bytes is [`redact::Finished::emit`].
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

use super::Document;
use super::extract::{self, ObjectStreams};
use super::handle::ObjectHandle;
use super::name::Name;
use super::resources::PageResources;
use super::sharing::{FormUseCounts, count_form_uses};
use crate::codes::qpdf::object_type;
use crate::pdfsyntax::geometry::{
    Glyph, check_form_sharing, check_type_three_procedure, glyphs_in, remove_glyphs,
    remove_glyphs_across,
};
use crate::pdfsyntax::region::{PageFrame, Region};
use crate::pdfsyntax::tounicode::ToUnicode;
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

/// One page's redaction, against a live qpdf document.
pub(crate) struct QpdfRedaction {
    document: Document,
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
}

impl QpdfRedaction {
    /// Begin a redaction of `page` over `region`.
    ///
    /// # Errors
    ///
    /// Whatever opening or walking the document failed with, including every refusal the walk
    /// and the sharing rule raise.
    pub(crate) fn new(
        document: Document,
        page: usize,
        region: Region,
        limits: burrow_types::Limits,
        deadline: Deadline,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        // THE INVARIANT `page_handle`'s SAFETY COMMENT RELIES ON, established here rather
        // than in one caller. It said "`self.page` was checked against the page count when the
        // redaction was built" and nothing in this constructor checked it -- the check lived in
        // `redact_page_for_probe`, and `new` is `pub(crate)`, so a second crate-internal caller
        // got undefined behaviour under a comment saying it could not happen.
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
        })
    }

    /// Every page this operation covers, as indices that exist in the document.
    ///
    /// **The page being edited is always in it**, even when the caller's set does not name it:
    /// its codes are the ones the content edit just changed, and a font cut without them is a
    /// font cut against a page nobody looked at. An index past the end is a refusal rather
    /// than a skip — `ObjectHandle::page` is unsafe on one, and a silently skipped page is a
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

    fn page_handle(&self) -> Result<ObjectHandle<'_>> {
        // SAFETY: `self.page` was checked against the page count when the redaction was built.
        let page = unsafe { ObjectHandle::page(&self.document, self.page) };
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        Ok(page)
    }

    /// The page's frame, for converting the region into content space.
    fn frame(&self, page: &ObjectHandle<'_>) -> Result<PageFrame> {
        super::redact_frame::of(&self.document, page)
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
    fn page_contents<'a>(
        &'a self,
        page: &ObjectHandle<'a>,
    ) -> Result<Option<(crate::pdfsyntax::contents::Contents, Vec<ObjectHandle<'a>>)>> {
        let contents = page.key(&CONTENTS);
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        let handles: Vec<ObjectHandle<'a>> = match contents.type_code() {
            object_type::STREAM => vec![contents],
            object_type::ARRAY => {
                let length = contents.array_len();
                if let Some(error) = self.document.take_error() {
                    return Err(error);
                }
                let mut out = Vec::new();
                for at in 0..length {
                    let element = contents.array_item(at);
                    if let Some(error) = self.document.take_error() {
                        return Err(error);
                    }
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
        // `QpdfRedaction::new`, so this function's bound was an ordering property of a
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

impl Steps for QpdfRedaction {
    fn affected_streams(&mut self) -> Result<Vec<StreamId>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
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

        let frame = self.frame(&page)?;
        let region = self.region.to_content_space(&frame)?;

        // EVERY GLYPH, then the ones the region reaches. The conservative box, not the advance
        // box: a glyph's ink can sit far from its origin, so uncertainty removes more.
        let glyphs = glyphs_in(contents.bytes(), &resources)?;
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
        let drawn_fonts: BTreeSet<Vec<u8>> = glyphs
            .iter()
            .map(|glyph| glyph.source.font.clone())
            .collect();
        check_type_three(&resources, &drawn_fonts)?;

        let mut streams: Vec<StreamId> = cut
            .iter()
            .map(|glyph| glyph.source.form.map_or(StreamId::Page, StreamId::Object))
            .collect();
        streams.sort_unstable();
        streams.dedup();
        drop(resources);
        drop(elements);
        drop(page);
        self.cut = cut;
        Ok(streams)
    }

    fn rewrite(&mut self, stream: StreamId) -> Result<()> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        let resources = PageResources::of(&page)?;
        let null = ObjectHandle::new_null(&self.document);

        match stream {
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
                let parts = remove_glyphs_across(&contents, None, &mine)?;
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
            }
            StreamId::Object(id) => {
                let form = find_form(&resources, id)?;
                let mine: Vec<Glyph> = self
                    .cut
                    .iter()
                    .filter(|glyph| glyph.source.form == Some(id))
                    .cloned()
                    .collect();
                let edited = remove_glyphs(&form, Some(id), &mine)?;
                // NO TYPE CHECK HERE, and its absence is deliberate. `form_handle` returns
                // only entries whose `type_code()` is `STREAM` and otherwise refuses with
                // `form-vanished`, so a check here could not fire -- a review planted a
                // mutation of it and nothing could fail. An unreachable guard reads as
                // coverage and is not any.
                let target = form_handle(&resources, id)?;
                target.replace_stream_data(&edited, &null, &null)?;
                drop(target);
            }
        }
        drop(resources);
        drop(page);
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
            self.deadline.checkpoint(self.clock.as_ref())?;
            // SAFETY: `pages_in_scope` yields only indices below the document's page count,
            // which it reads from the document itself.
            let page = unsafe { ObjectHandle::page(&self.document, at) };
            if let Some(error) = self.document.take_error() {
                return Err(error);
            }
            let content = page.page_content()?;
            let resources = PageResources::of(&page)?;
            for glyph in &glyphs_in(&content, &resources)? {
                // KEYED BY THE FONT'S OBJECT, not its resource name: two names can mean one
                // object and one name can mean different objects on different pages, and font
                // surgery edits objects. `core/CLAUDE.md`'s identity rule, one level up.
                let font = resources.font_object(&glyph.source.font)?;
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
    ) -> Result<Vec<FontOutcome>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        let resources = PageResources::of(&page)?;

        let mut outcomes = Vec::new();

        for name in font_names(&resources)? {
            let font = resources.dictionary().key(&FONT).key(&name);
            if font.type_code() != object_type::DICTIONARY {
                continue;
            }
            let identity = font.object()?;
            let packed = pack(identity);
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
            narrow_font(&self.document, &font, &keeps)?;
            outcomes.push(FontOutcome {
                font: packed,
                cut: true,
                also_used_by: outside,
            });
        }
        Ok(outcomes)
    }

    fn strip_page_keys(&mut self) -> Result<()> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        // ADR 0029 §2's allowlist, shared with `prune` rather than written again: two lists
        // that are supposed to agree are two lists that can disagree.
        for key in crate::prune::page_keys_outside_the_allowlist(&page.unparse())? {
            page.remove_key(&Name::from_stripped(&key)?);
            if let Some(error) = self.document.take_error() {
                return Err(error);
            }
        }
        let frame = self.frame(&page)?;
        let region = self.region.to_content_space(&frame)?;
        remove_annotations_in(&self.document, &page, &region)?;
        Ok(())
    }

    fn write(&mut self) -> Result<Vec<u8>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        extract::write_out(&self.document, &self.document, ObjectStreams::Preserve)
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
/// [`ObjectHandle::erase_item`] renumbers, so a forward loop removing item 2 of 5 makes the old
/// item 3 the new item 2 and never examines it — the same defect `prune`'s `/Annots` walk was
/// written backwards to avoid, and for the same reason: a skipped annotation is one whose
/// appearance nobody looked at.
///
/// An annotation whose `/Rect` is absent or not four numbers is refused rather than kept: where
/// it sits is then unknown, and unknown is not "outside the region".
fn remove_annotations_in(
    document: &Document,
    page: &ObjectHandle<'_>,
    region: &crate::pdfsyntax::geometry::Rect,
) -> Result<()> {
    const ANNOTS: Name = Name::literal(b"/Annots\0");
    const RECT: Name = Name::literal(b"/Rect\0");

    let annots = page.key(&ANNOTS);
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    if annots.type_code() != object_type::ARRAY {
        return Ok(());
    }
    let length = annots.array_len();
    if length > DIFFERENCES_CEILING {
        return Err(Error::Unsupported(
            "pdf redaction [annots-too-long]: an /Annots array longer than burrow will examine"
                .to_owned(),
        ));
    }
    for at in (0..length).rev() {
        let annotation = annots.array_item(at);
        if annotation.type_code() != object_type::DICTIONARY {
            // Not an annotation. `prune`'s walk found these too, and the container type check
            // is what stops each one retaining a qpdf warning.
            continue;
        }
        let rect = annotation.key(&RECT);
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        let numbers = crate::pdfsyntax::ops::numbers_in(&rect.unparse());
        let [left, bottom, right, top] = numbers.as_slice() else {
            return Err(Error::Malformed(
                "pdf redaction [annotation-rect]: an annotation whose /Rect is not four \
                 numbers, so where it draws is unknown"
                    .to_owned(),
            ));
        };
        // NORMALISED. `/Rect`'s corners are in either order per PDF 32000-1 §12.5.2, and an
        // un-normalised rectangle compares as empty against every region.
        let box_of = crate::pdfsyntax::geometry::Rect {
            left: left.min(*right),
            bottom: bottom.min(*top),
            right: left.max(*right),
            top: bottom.max(*top),
        };
        if !box_of.is_finite() {
            return Err(Error::Malformed(
                "pdf redaction [annotation-rect]: an annotation whose /Rect is not finite"
                    .to_owned(),
            ));
        }
        if box_of.intersects(region) {
            annots.erase_item(at);
            if let Some(error) = document.take_error() {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// An object identity packed into the `u64` `Form::id` and `FontOutcome::font` use.
const fn pack(identity: (core::ffi::c_int, core::ffi::c_int)) -> u64 {
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
fn check_contents_sharing(
    cut: &[Glyph],
    contents: &crate::pdfsyntax::contents::Contents,
    elements: &[ObjectHandle<'_>],
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
fn check_type_three(resources: &PageResources<'_>, drawn: &BTreeSet<Vec<u8>>) -> Result<()> {
    const SUBTYPE: Name = Name::literal(b"/Subtype\0");
    const TYPE_THREE: Name = Name::literal(b"/Type3\0");
    const CHAR_PROCS: Name = Name::literal(b"/CharProcs\0");

    for font_name in drawn {
        let font = resources.font_handle(font_name)?;
        let subtype = font.key(&SUBTYPE);
        if subtype.type_code() != object_type::NAME || subtype.name()? != TYPE_THREE {
            continue;
        }
        let procs = font.key(&CHAR_PROCS);
        if procs.type_code() != object_type::DICTIONARY {
            continue;
        }
        for key in crate::pdfsyntax::dict::top_level_keys(&procs.unparse())? {
            let entry = procs.key(&Name::from_stripped(&key)?);
            if entry.type_code() != object_type::STREAM {
                continue;
            }
            let Some(procedure) = entry.stream_data()? else {
                // Undecodable, so what it draws is unknown, and unknown is not "no text".
                return Err(Error::Malformed(
                    "pdf redaction [type-three-unreadable]: a Type 3 glyph procedure whose data \
                     burrow could not decode"
                        .to_owned(),
                ));
            };
            check_type_three_procedure(&procedure)?;
        }
    }
    Ok(())
}

/// Every `/Font` resource name on the page./// Every `/Font` resource name on the page.
fn font_names(resources: &PageResources<'_>) -> Result<Vec<Name>> {
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
fn find_form(resources: &PageResources<'_>, id: u64) -> Result<Vec<u8>> {
    form_handle(resources, id)?.stream_data()?.ok_or_else(|| {
        Error::Malformed(
            "pdf redaction [form-unreadable]: a Form XObject whose data burrow could not \
                 decode"
                .to_owned(),
        )
    })
}

/// The handle for the form with this identity.
fn form_handle<'a>(resources: &PageResources<'a>, id: u64) -> Result<ObjectHandle<'a>> {
    const XOBJECT: Name = Name::literal(b"/XObject\0");
    let xobjects = resources.dictionary().key(&XOBJECT);
    for key in crate::pdfsyntax::dict::top_level_keys(&xobjects.unparse())? {
        let entry = xobjects.key(&Name::from_stripped(&key)?);
        if entry.type_code() == object_type::STREAM && pack(entry.object()?) == id {
            return Ok(entry);
        }
    }
    Err(Error::Malformed(
        "pdf redaction [form-vanished]: a form the walk found is not in the page's resources"
            .to_owned(),
    ))
}

/// Remove a font's entries for codes the document no longer draws.
///
/// **Narrowed, not deleted.** Both of the first two edits here started as a removal of the
/// whole structure, and both were wrong in the same way: `/ToUnicode` and `/Differences` say
/// what the *kept* codes are as well as what the removed ones were, so deleting them damages
/// the text the redaction was meant to leave alone. `/ToUnicode` was measured — PDFium read
/// `U+0001` at the origin where readable text had been. `/Differences` was not measured; it is
/// the same shape, found by looking again at the other half of the same function.
fn narrow_font(document: &Document, font: &ObjectHandle<'_>, keeps: &BTreeSet<u32>) -> Result<()> {
    narrow_to_unicode(document, font, keeps)?;
    narrow_differences(document, font, keeps)?;

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
                widths.set_array_item(at, &ObjectHandle::new_integer(document, 0));
            }
        }
    }
    // DRAINED, and it was not. The `/Widths` loop ran after both narrowings had drained and
    // then returned `Ok(())` with nothing of its own — so a failure writing a width surfaced
    // at the NEXT `take_error`: a different font's `/ToUnicode`, or the page strip, or the
    // write. A wrong answer attributed to the wrong cause, which is the failure mode
    // `prune.rs`'s drain-after-every-call rule exists for.
    drained(document)
}

/// Rewrite `/ToUnicode` so it maps the kept codes and no others.
///
/// `/ToUnicode` IS THE REMOVED CHARACTER, in plain text, beside the page it was cut from, so
/// leaving it is not an option. Deleting it is not one either: see this function's caller.
/// `qpdf_oh_replace_stream_data` is trapped (ADR 0013), so the narrowed program can be written
/// back, and the filter arguments are nulls — the stream is stored uncompressed, which the
/// write path's `ObjectStreams::Preserve` does not undo. A `/ToUnicode` is a few kilobytes.
fn narrow_to_unicode(
    document: &Document,
    font: &ObjectHandle<'_>,
    keeps: &BTreeSet<u32>,
) -> Result<()> {
    let to_unicode = font.key(&TO_UNICODE);
    if to_unicode.type_code() != object_type::STREAM {
        return Ok(());
    }
    let Some(program) = to_unicode.stream_data()? else {
        // Undecodable: burrow cannot say what it maps, so it cannot say it is safe to keep.
        // Removing it loses the kept codes' text, which is a legibility cost rather than a
        // leak, and is the only honest answer available here.
        font.remove_key(&TO_UNICODE);
        return drained(document);
    };
    let read = ToUnicode::parse(&program)?;
    match read.narrowed(&|code| keeps.contains(&code)) {
        Some(narrowed) => {
            let null = ObjectHandle::new_null(document);
            to_unicode.replace_stream_data(&narrowed, &null, &null)?;
        }
        // Nothing the CMap mapped is still drawn, so there is no text to preserve and a CMap
        // with no `bfchar` section is not a CMap.
        None => font.remove_key(&TO_UNICODE),
    }
    drained(document)
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
/// Erasing one name is not available either: [`ObjectHandle::erase_item`] renumbers, so every
/// later name would slide onto the wrong code. And building a replacement array is closed —
/// `qpdf_oh_new_array` and `qpdf_oh_new_name` are untrapped and do not meet
/// `engines/qpdf-untrapped-accepted.toml`'s non-parsing bar.
///
/// What is available is [`ObjectHandle::set_array_item`] with an integer, and it happens to be
/// exactly right. Replacing the name at index `i`, whose code is `c`, with the integer `c + 1`
/// leaves the array the same length and re-anchors the run at the code the next item already
/// had. `[1 /a /b /c]` with `/b` removed becomes `[1 /a 3 /c]`, and `/c` is still code 3.
fn narrow_differences(
    document: &Document,
    font: &ObjectHandle<'_>,
    keeps: &BTreeSet<u32>,
) -> Result<()> {
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
            continue;
        }
        let survives = keeps.contains(&code) && last_at.get(&code) == Some(&at);
        if !survives {
            let next = i64::from(code).saturating_add(1);
            differences.set_array_item(at, &ObjectHandle::new_integer(document, next));
        }
        code = code.saturating_add(1);
    }
    drained(document)
}

/// Walk a `/Differences` array, calling `seen` with each name's code and index.
fn walk_differences(
    differences: &ObjectHandle<'_>,
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
            continue;
        }
        seen(code, at);
        code = code.saturating_add(1);
    }
    Ok(())
}

/// A `/Differences` anchor, refused rather than folded onto zero.
///
/// It was `u32::try_from(value).unwrap_or(0)`, which is the silent truncation `core/CLAUDE.md`
/// denies `as` casts for, spelled differently. An anchor of `-1` or `2^32` became code 0, so
/// every name after it was tested against the wrong code — and a glyph name spelling removed
/// text stayed in the file because its mis-computed code happened to be kept. Measured: a
/// `/Differences` of `[-1 /S /e /c /r]` came back unchanged.
fn differences_anchor(item: &ObjectHandle<'_>) -> Result<u32> {
    u32::try_from(item.integer_value()).map_err(|_| {
        Error::Malformed(
            "pdf redaction [differences-anchor]: a /Differences array anchored at a code              outside the range a character code can take"
                .to_owned(),
        )
    })
}

/// Drain whatever qpdf latched, as an error.
fn drained(document: &Document) -> Result<()> {
    document.take_error().map_or(Ok(()), Err)
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
fn first_char(font: &ObjectHandle<'_>) -> Result<u32> {
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
