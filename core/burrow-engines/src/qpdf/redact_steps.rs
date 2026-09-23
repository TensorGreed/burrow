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
//! # Crate-internal, per ADR 0022
//!
//! Nothing here is exported. There is no caller-visible redaction until #134's verification
//! exists, and `redact_probe` — the one narrow seam, returning geometry rather than bytes —
//! goes when it lands.

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
    /// Counted once, before any edit: the sharing rule is about the document as it arrived.
    sharing: FormUseCounts,
    deadline: Deadline,
    clock: Arc<dyn Clock>,
    /// The glyphs the region reaches, resolved once by `affected_streams`.
    cut: Vec<Glyph>,
    /// Each affected stream's rewritten bytes, so `codes_still_drawn` reads the finished
    /// content rather than re-deriving it.
    rewritten: BTreeMap<StreamId, Vec<u8>>,
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
        deadline: Deadline,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        // COUNTED BEFORE ANY EDIT. The sharing rule is about the document as it arrived; a
        // count taken after a rewrite would be counting the operation's own work.
        let sharing = count_form_uses(&document, &deadline, &clock)?;
        Ok(Self {
            document,
            page,
            region,
            sharing,
            deadline,
            clock,
            cut: Vec::new(),
            rewritten: BTreeMap::new(),
        })
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
}

impl Steps for QpdfRedaction {
    fn affected_streams(&mut self) -> Result<Vec<StreamId>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        let content = page.page_content()?;
        let resources = PageResources::of(&page)?;

        let frame = self.frame(&page)?;
        let region = self.region.to_content_space(&frame)?;

        // EVERY GLYPH, then the ones the region reaches. The conservative box, not the advance
        // box: a glyph's ink can sit far from its origin, so uncertainty removes more.
        let glyphs = glyphs_in(&content, &resources)?;
        let cut: Vec<Glyph> = glyphs
            .into_iter()
            .filter(|glyph| glyph.conservative_box().intersects(&region))
            .collect();

        // THE SHARING RULE, before any edit is planned. A form the region reaches that is
        // drawn elsewhere is refused rather than edited -- editing it removes its text from a
        // page nobody selected, which §6's read-back cannot see.
        check_form_sharing(&cut, &self.sharing)?;

        // THE TYPE 3 RULE, the same shape and for the same reason. A glyph procedure is a
        // content stream the walk does not descend into, so text inside one is text nothing
        // observed -- ADR 0029 §8's forbidden outcome. `check_type_three_procedure` refuses a
        // procedure that shows any text; this is the part that finds the procedures to hand it.
        check_type_three(&resources, &cut)?;

        let mut streams: Vec<StreamId> = cut
            .iter()
            .map(|glyph| glyph.source.form.map_or(StreamId::Page, StreamId::Object))
            .collect();
        streams.sort_unstable();
        streams.dedup();
        drop(resources);
        drop(page);
        self.cut = cut;
        Ok(streams)
    }

    fn rewrite(&mut self, stream: StreamId) -> Result<()> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        let page = self.page_handle()?;
        let resources = PageResources::of(&page)?;

        let (content, form) = match stream {
            StreamId::Page => (page.page_content()?, None),
            StreamId::Object(id) => {
                let form = find_form(&resources, id)?;
                (form, Some(id))
            }
        };
        let mine: Vec<Glyph> = self
            .cut
            .iter()
            .filter(|glyph| glyph.source.form == form)
            .cloned()
            .collect();
        let edited = remove_glyphs(&content, form, &mine)?;

        // WRITTEN BACK THROUGH THE TRAPPED VERB, with a null filter: the replacement is plain
        // bytes, and a null `/Filter` is how "no filter" is said. Verified end to end against
        // qpdf 12.4.1 -- the stream comes back re-compressed with a correct `/Length`.
        let target = match stream {
            StreamId::Page => page.key(&CONTENTS),
            StreamId::Object(id) => form_handle(&resources, id)?,
        };
        if target.type_code() != object_type::STREAM {
            return Err(Error::Malformed(
                "pdf redaction [contents-not-a-stream]: a page whose /Contents is not a single \
                 stream, which this operation does not yet rewrite"
                    .to_owned(),
            ));
        }
        let null = ObjectHandle::new_null(&self.document);
        target.replace_stream_data(&edited, &null, &null)?;
        drop(target);
        drop(resources);
        drop(page);
        self.rewritten.insert(stream, edited);
        Ok(())
    }

    fn codes_still_drawn(&mut self) -> Result<Vec<(u64, Vec<u32>)>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        // THE FINISHED CONTENT, which is why this step cannot run before the rewrites. The
        // `/ToUnicode` entry for a removed glyph IS the removed character; a font cut from a
        // snapshot taken part-way would keep entries for codes a later edit removed.
        let page = self.page_handle()?;
        let content = page.page_content()?;
        let resources = PageResources::of(&page)?;
        let glyphs = glyphs_in(&content, &resources)?;

        let mut drawn: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
        for glyph in &glyphs {
            // KEYED BY THE FONT'S OBJECT, not its resource name: two names can mean one
            // object and one name can mean different objects on different pages, and font
            // surgery edits objects. `core/CLAUDE.md`'s identity rule, one level up.
            let font = resources.font_object(&glyph.source.font)?;
            drawn.entry(font).or_default().insert(glyph.source.code);
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

        let cuttable = self.sharing.fonts_wholly_within(redacted);
        let pages = self.sharing.font_pages_map();
        let mut outcomes = Vec::new();

        for name in font_names(&resources)? {
            let font = resources.dictionary().key(&FONT).key(&name);
            if font.type_code() != object_type::DICTIONARY {
                continue;
            }
            let identity = font.object()?;
            let packed = pack(identity);
            let outside = pages
                .get(&identity)
                .map_or(0, |used| used.difference(redacted).count());

            if !cuttable.contains(&identity) {
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
                also_used_by: 0,
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
        Ok(())
    }

    fn write(&mut self) -> Result<Vec<u8>> {
        self.deadline.checkpoint(self.clock.as_ref())?;
        extract::write_out(&self.document, &self.document, ObjectStreams::Preserve)
    }
}

/// An object identity packed into the `u64` `Form::id` and `FontOutcome::font` use.
const fn pack(identity: (core::ffi::c_int, core::ffi::c_int)) -> u64 {
    // `unsigned_abs`, not `as`: this crate denies sign-losing casts, and an object number is
    // never negative in a document qpdf opened. The same expression as
    // `PageResources::font_object`, because the two pack the same thing and a packing that
    // disagreed with itself would make one font look like two.
    ((identity.0.unsigned_abs() as u64) << 16) | (identity.1.unsigned_abs() as u64 & 0xffff)
}

/// Refuse if any cut glyph's Type 3 procedure shows text of its own.
///
/// # It resolves the specific procedure, and falls back to all of them
///
/// A Type 3 font maps a code to a `/CharProcs` name through its `/Encoding /Differences`, so
/// the procedure for a cut glyph can be named exactly — and a font whose *other* glyphs draw
/// text is then not refused for glyphs the region never reached.
///
/// When the font has no readable `/Differences`, there is no code-to-name map to follow, so
/// every procedure is scanned. That over-refuses, which is the direction that does not leak:
/// the alternative is deciding a procedure is harmless without having looked at it.
fn check_type_three(resources: &PageResources<'_>, cut: &[Glyph]) -> Result<()> {
    const SUBTYPE: Name = Name::literal(b"/Subtype\0");
    const TYPE_THREE: Name = Name::literal(b"/Type3\0");
    const CHAR_PROCS: Name = Name::literal(b"/CharProcs\0");

    let mut seen: BTreeSet<(Vec<u8>, u32)> = BTreeSet::new();
    for glyph in cut {
        if !seen.insert((glyph.source.font.clone(), glyph.source.code)) {
            continue;
        }
        let font = resources.font_handle(&glyph.source.font)?;
        let subtype = font.key(&SUBTYPE);
        if subtype.type_code() != object_type::NAME || subtype.name()? != TYPE_THREE {
            continue;
        }
        let procs = font.key(&CHAR_PROCS);
        if procs.type_code() != object_type::DICTIONARY {
            continue;
        }
        let named = char_proc_name(&font, glyph.source.code)?;
        for key in crate::pdfsyntax::dict::top_level_keys(&procs.unparse())? {
            let name = Name::from_stripped(&key)?;
            if named.as_ref().is_some_and(|wanted| *wanted != name) {
                continue;
            }
            let entry = procs.key(&name);
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

/// The `/CharProcs` name a Type 3 font's `/Encoding /Differences` gives `code`.
fn char_proc_name(font: &ObjectHandle<'_>, code: u32) -> Result<Option<Name>> {
    let encoding = font.key(&ENCODING);
    if encoding.type_code() != object_type::DICTIONARY {
        return Ok(None);
    }
    let differences = encoding.key(&DIFFERENCES);
    if differences.type_code() != object_type::ARRAY {
        return Ok(None);
    }
    let mut at_code: u32 = 0;
    for at in 0..differences.array_len() {
        let item = differences.array_item(at);
        if item.type_code() == object_type::INTEGER {
            at_code = u32::try_from(item.integer_value()).unwrap_or(0);
            continue;
        }
        if item.type_code() != object_type::NAME {
            continue;
        }
        if at_code == code {
            return Ok(Some(item.name()?));
        }
        at_code = at_code.saturating_add(1);
    }
    Ok(None)
}

/// Every `/Font` resource name on the page.
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
        let first = first_char(font);
        let length = widths.array_len();
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
    Ok(())
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
    let mut code: u32 = 0;
    for at in 0..length {
        let item = differences.array_item(at);
        if item.type_code() == object_type::INTEGER {
            code = u32::try_from(item.integer_value()).unwrap_or(0);
            continue;
        }
        if item.type_code() != object_type::NAME {
            continue;
        }
        if !keeps.contains(&code) {
            let next = i64::from(code).saturating_add(1);
            differences.set_array_item(at, &ObjectHandle::new_integer(document, next));
        }
        code = code.saturating_add(1);
    }
    drained(document)
}

/// Drain whatever qpdf latched, as an error.
fn drained(document: &Document) -> Result<()> {
    document.take_error().map_or(Ok(()), Err)
}

fn first_char(font: &ObjectHandle<'_>) -> u32 {
    const FIRST_CHAR: Name = Name::literal(b"/FirstChar\0");
    let value = font.key(&FIRST_CHAR);
    if value.type_code() == object_type::INTEGER {
        u32::try_from(value.integer_value()).unwrap_or(0)
    } else {
        0
    }
}
