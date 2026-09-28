//! Reading a redacted document back, through a fresh document of the same engine.
//!
//! The engine half of [`crate::redact_verify`]; the policy is there and this is the reading. It
//! shares no state with the redaction that produced the bytes: a new document, a new page tree,
//! opened from the emitted bytes and nothing else. Written once over
//! [`crate::redact::graph`] (#191), so the web engine reads back through the same code. Its
//! tests against the real engine are native, and live in `qpdf::redact_witness_tests`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use burrow_types::{Clock, Deadline, Limits, Result};

use super::resources::PageResources;
use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::geometry::{Glyph, ScopedFont, Watch, glyphs_in};
use crate::pdfsyntax::region::PageFrame;
use crate::pdfsyntax::tounicode::ToUnicode;
use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};
use crate::redact_verify::ClearedWitness;

/// An engine that opens emitted bytes and answers the read-back questions.
pub(crate) struct Witness<E> {
    engine: E,
    limits: Limits,
    clock: Arc<dyn Clock>,
    /// **The operation's deadline, not a fresh one.**
    ///
    /// `open_document` returns a freshly started `Deadline`, and `Deadline::start` resets the
    /// origin *and* the budget — so a witness spending that one gave the read-back a second
    /// full `max_duration_ms`. `burrow_ops::verify::output` says exactly this in capitals and
    /// `qpdf/rotate.rs` records it as a security-review finding. This module had it wrong, and
    /// ADR 0029's amendment called the deadline "shared with the edit" when it was not.
    ///
    /// `Deadline` is `Copy` — a start instant and a budget — so carrying it by value spends the
    /// same window rather than a new one.
    deadline: Deadline,
}

impl<E: OpensForRedaction + Clone> Witness<E> {
    /// The operation's deadline, as the geometry walk reads it during the read-back.
    ///
    /// The only place the read-back builds one; see `PageRedaction::watch` for why that matters.
    fn watch<'a>(&'a self, read: &ReadBack<E::Document>) -> Watch<'a> {
        Watch::new(read.deadline, self.clock.as_ref())
    }
}

impl<E> Witness<E> {
    /// A witness over `engine`, spending the operation's ceilings and remaining time.
    pub(crate) const fn over(
        engine: E,
        limits: Limits,
        clock: Arc<dyn Clock>,
        deadline: Deadline,
    ) -> Self {
        Self {
            engine,
            limits,
            clock,
            deadline,
        }
    }
}

/// A document opened for reading back, with the deadline its reads spend.
pub(crate) struct ReadBack<D> {
    document: D,
    deadline: Deadline,
}

impl<D: PdfDocument> ReadBack<D> {
    /// The page handle for `page`, bounds-checked.
    fn page(&self, page: usize) -> Result<D::Object<'_>> {
        let count = usize::try_from(self.document.page_count()?).unwrap_or(0);
        if page >= count {
            return Err(burrow_types::Error::OutputRejected(format!(
                "redact: the output has {count} page(s) and page {page} was verified"
            )));
        }
        self.document.page(page)
    }

    /// The page's content, as one lexical stream.
    fn content(&self, page: usize) -> Result<Vec<u8>> {
        self.page(page)?.page_content()
    }
}

impl<E: OpensForRedaction + Clone> ClearedWitness for Witness<E> {
    type Read = ReadBack<E::Document>;

    fn fresh(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            limits: self.limits,
            clock: Arc::clone(&self.clock),
            deadline: self.deadline,
        }
    }

    fn open_output(&self, bytes: &[u8]) -> Result<Self::Read> {
        // A HOOK THAT MAKES THE READ-BACK FAIL, so a test can ask whether the operation
        // propagates a rejection at all.
        //
        // No correct operation emits a document that fails its own check, so nothing a
        // *document* can do exercises "the verification result is used". A mutation replacing
        // the closure's last line with `let _ = …; Ok(())` therefore survived the whole suite --
        // discarding the central claim of #134 with nothing standing behind it. The hook is in
        // the WITNESS rather than in the closure on purpose: a hook in the closure would be
        // bypassed by exactly that mutation.
        #[cfg(test)]
        if let Some(error) = crate::redact::hooks::forced_read_back_failure() {
            return Err(error);
        }
        let options = crate::OpenOptions::new(self.limits, Arc::clone(&self.clock));
        // THE OPEN'S OWN DEADLINE IS A FRESHLY STARTED ONE and it is discarded;
        // `input_rotations` does the same for the same reason. See `Witness::deadline`.
        let (document, _fresh) = self.engine.open_for_redaction(bytes, &options)?;
        Ok(ReadBack {
            document,
            deadline: self.deadline,
        })
    }

    fn frame_of(&self, read: &Self::Read, page: usize) -> Result<PageFrame> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        super::frame::of(&read.page(page)?)
    }

    fn glyphs_on(&self, read: &Self::Read, page: usize) -> Result<Vec<Glyph>> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        let handle = read.page(page)?;
        let content = read.content(page)?;
        let resources = PageResources::of(&handle)?;
        glyphs_in(&content, &resources, &self.watch(read))
    }

    fn orphaned_codes(
        &self,
        read: &Self::Read,
        page: usize,
        cut: &BTreeSet<crate::redact_verify::FontPath>,
        drawn: &BTreeMap<u64, BTreeSet<u32>>,
    ) -> Result<usize> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        let handle = read.page(page)?;
        let resources = PageResources::of(&handle)?;
        // THE PARSED MAP IS MEMOISED, NEVER ITS DOMAIN: two fonts sharing one `/ToUnicode` parse
        // it once, and nothing here holds 65,536 codes per font.
        let mut parsed: BTreeMap<(core::ffi::c_int, core::ffi::c_int), std::rc::Rc<ToUnicode>> =
            BTreeMap::new();
        let mut examined: BTreeSet<u64> = BTreeSet::new();
        let mut orphaned = 0;
        let nothing = BTreeSet::new();
        for path in cut {
            // CHECKPOINTED PER FONT: the trip count is the operation's, which is the file's.
            read.deadline.checkpoint(self.clock.as_ref())?;
            let font = resources.font_at(path)?;
            let identity = font.object()?;
            // THE SECOND LINE BEHIND `[direct-font]`: a font with no identity cannot be keyed
            // against what it draws, because every such font shares the key.
            if identity == (0, 0) {
                return Err(burrow_types::Error::OutputRejected(
                    "redact: the region is not cleared -- a cut font has no identity to check it by"
                        .to_owned(),
                ));
            }
            let key = super::steps::pack(identity);
            if !examined.insert(key) {
                continue;
            }
            let still = drawn.get(&key).unwrap_or(&nothing);
            orphaned += orphans_of(&font, still, &mut parsed)?;
        }
        Ok(orphaned)
    }

    fn drawn_codes(&self, read: &Self::Read, page: usize) -> Result<BTreeMap<u64, BTreeSet<u32>>> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        let handle = read.page(page)?;
        let content = read.content(page)?;
        let resources = PageResources::of(&handle)?;
        let mut drawn: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
        let mut resolved: BTreeMap<ScopedFont, u64> = BTreeMap::new();
        for glyph in &glyphs_in(&content, &resources, &self.watch(read))? {
            // IN THE SCOPE THAT DREW IT. The read-back resolved against the page too, so
            // it refused documents the redaction had handled correctly. ONCE PER FONT AND
            // ROUTE, not per glyph: see `Steps::codes_still_drawn`.
            let font = match resolved.get(&glyph.source.font) {
                Some(&font) => font,
                None => {
                    let font =
                        super::steps::pack(resources.font_in_scope(&glyph.source.font)?.object()?);
                    resolved.insert(glyph.source.font.clone(), font);
                    font
                }
            };
            drawn.entry(font).or_default().insert(glyph.source.code);
        }
        Ok(drawn)
    }

    fn page_keys(&self, read: &Self::Read, page: usize) -> Result<Vec<Vec<u8>>> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        crate::pdfsyntax::dict::top_level_keys(&read.page(page)?.unparse())
    }
}

/// How many codes `font`'s `/ToUnicode` and `/Differences` name that are not in `still`, read the
/// way the narrowing writes them so the read-back and the operation agree on what "mapped" means.
///
/// Counted, never collected: a code in both halves counts once.
///
/// # Errors
///
/// [`burrow_types::Error::OutputRejected`] for a `/ToUnicode` whose program cannot be read: a
/// domain that cannot be established cannot be checked. [`burrow_types::Error::Malformed`] from
/// parsing it. Whatever reading the font refused.
fn orphans_of<O: PdfObject>(
    font: &O,
    still: &BTreeSet<u32>,
    parsed: &mut BTreeMap<(core::ffi::c_int, core::ffi::c_int), std::rc::Rc<ToUnicode>>,
) -> Result<usize> {
    const TO_UNICODE: Name = Name::literal(b"/ToUnicode\0");
    const ENCODING: Name = Name::literal(b"/Encoding\0");
    const DIFFERENCES: Name = Name::literal(b"/Differences\0");

    let mut map: Option<std::rc::Rc<ToUnicode>> = None;
    let to_unicode = font.key(&TO_UNICODE);
    if to_unicode.type_code() == object_type::STREAM {
        let identity = to_unicode.object()?;
        // A DIRECT STREAM HAS NO IDENTITY -- qpdf reports `(0, 0)` for every one -- so it is
        // never taken from the memo.
        if let Some(already) = parsed.get(&identity).filter(|_| identity != (0, 0)) {
            map = Some(std::rc::Rc::clone(already));
        } else {
            let Some(program) = to_unicode.stream_data()? else {
                return Err(burrow_types::Error::OutputRejected(
                    "redact: the region is not cleared -- a cut font's /ToUnicode cannot be read back"
                        .to_owned(),
                ));
            };
            let read_map = std::rc::Rc::new(ToUnicode::parse(&program)?);
            parsed.insert(identity, std::rc::Rc::clone(&read_map));
            map = Some(read_map);
        }
    }
    let mut orphaned = 0;
    if let Some(map) = &map {
        orphaned += (0..=u32::from(u16::MAX))
            .filter(|code| map.maps(*code) && !still.contains(code))
            .count();
    }
    // `/Differences`' names, by the same walk `narrow_differences` makes.
    let encoding = font.key(&ENCODING);
    if encoding.type_code() == object_type::DICTIONARY {
        let differences = encoding.key(&DIFFERENCES);
        if differences.type_code() == object_type::ARRAY {
            let mut code: u32 = 0;
            for at in 0..differences.array_len() {
                let item = differences.array_item(at);
                if item.type_code() == object_type::INTEGER {
                    // SKIPPED, NOT FOLDED TO ZERO. `narrow_differences` refuses an anchor outside
                    // a character code; folding it here would register a mapped code 0 that is
                    // not there and reject the document for it.
                    let Ok(anchor) = u32::try_from(item.integer_value()) else {
                        continue;
                    };
                    code = anchor;
                    continue;
                }
                if item.type_code() != object_type::NAME {
                    continue;
                }
                let counted_already = map.as_ref().is_some_and(|m| m.maps(code));
                if !still.contains(&code) && !counted_already {
                    orphaned += 1;
                }
                code = code.saturating_add(1);
            }
        }
    }
    // THE FONT'S DOCUMENT, which is the read-back's: the drain goes through the handle that did
    // the reading (#191).
    font.drained()?;
    Ok(orphaned)
}
