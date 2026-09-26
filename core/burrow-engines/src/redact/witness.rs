//! Reading a redacted document back, through a fresh document of the same engine.
//!
//! The engine half of [`crate::redact_verify`]; the policy is there and this is the reading. It
//! shares no state with the redaction that produced the bytes: a new document, a new page tree,
//! opened from the emitted bytes and nothing else. Written once over
//! [`crate::redact::graph`] (#191), so the web half will read back through the same code. Its
//! tests against the real engine are native, and live in `qpdf::redact_witness_tests`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use burrow_types::{Clock, Deadline, Limits, Result};

use super::resources::PageResources;
use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::geometry::{Glyph, Watch, glyphs_in};
use crate::pdfsyntax::region::PageFrame;
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

    fn drawn_codes(&self, read: &Self::Read, page: usize) -> Result<BTreeMap<u64, BTreeSet<u32>>> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        let handle = read.page(page)?;
        let content = read.content(page)?;
        let resources = PageResources::of(&handle)?;
        let mut drawn: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
        for glyph in &glyphs_in(&content, &resources, &self.watch(read))? {
            // IN THE SCOPE THAT DREW IT. The read-back resolved against the page too, so
            // it refused documents the redaction had handled correctly.
            let font = super::steps::pack(resources.font_in_scope(&glyph.source.font)?.object()?);
            drawn.entry(font).or_default().insert(glyph.source.code);
        }
        Ok(drawn)
    }

    fn mapped_codes(&self, read: &Self::Read, page: usize) -> Result<BTreeMap<u64, BTreeSet<u32>>> {
        const FONT: Name = Name::literal(b"/Font\0");
        const TO_UNICODE: Name = Name::literal(b"/ToUnicode\0");
        const ENCODING: Name = Name::literal(b"/Encoding\0");
        const DIFFERENCES: Name = Name::literal(b"/Differences\0");

        read.deadline.checkpoint(self.clock.as_ref())?;
        let handle = read.page(page)?;
        let resources = PageResources::of(&handle)?;
        let fonts = resources.dictionary().key(&FONT);
        if fonts.type_code() != object_type::DICTIONARY {
            return Ok(BTreeMap::new());
        }

        let mut mapped: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
        // MEMOISED PER STREAM OBJECT. Two fonts sharing one `/ToUnicode` parsed it twice, and
        // the `/Font` dictionary's size is the file's to choose -- the same shape a security
        // review measured at 19.8 s against a 100 ms budget in the operation's own font loop.
        let mut parsed: BTreeMap<(core::ffi::c_int, core::ffi::c_int), BTreeSet<u32>> =
            BTreeMap::new();
        for key in crate::pdfsyntax::dict::top_level_keys(&fonts.unparse())? {
            // CHECKPOINTED PER FONT, not once for the loop. The trip count comes from the
            // file, which is the definition of a loop that needs one inside it.
            read.deadline.checkpoint(self.clock.as_ref())?;
            let font = fonts.key(&Name::from_stripped(&key)?);
            if font.type_code() != object_type::DICTIONARY {
                continue;
            }
            let identity = font.object()?;
            let packed = (u64::from(identity.0.unsigned_abs()) << 16)
                | u64::from(identity.1.unsigned_abs() & 0xffff);
            let codes = mapped.entry(packed).or_default();

            // `/ToUnicode`'s domain, read the same way the narrowing writes it.
            let to_unicode = font.key(&TO_UNICODE);
            if to_unicode.type_code() == object_type::STREAM {
                let identity = to_unicode.object()?;
                if let Some(already) = parsed.get(&identity) {
                    codes.extend(already.iter().copied());
                } else if let Some(program) = to_unicode.stream_data()? {
                    let read_map = crate::pdfsyntax::tounicode::ToUnicode::parse(&program)?;
                    let mut domain = BTreeSet::new();
                    for code in 0..=u32::from(u16::MAX) {
                        if read_map.maps(code) {
                            domain.insert(code);
                        }
                    }
                    codes.extend(domain.iter().copied());
                    parsed.insert(identity, domain);
                }
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
                            // SKIPPED, NOT FOLDED TO ZERO. `narrow_differences` refuses an
                            // anchor outside a character code; folding it here would register
                            // a mapped code 0 that is not there and reject the document for it.
                            // Two readings of one array that are supposed to agree.
                            let Ok(anchor) = u32::try_from(item.integer_value()) else {
                                continue;
                            };
                            code = anchor;
                            continue;
                        }
                        if item.type_code() != object_type::NAME {
                            continue;
                        }
                        codes.insert(code);
                        code = code.saturating_add(1);
                    }
                }
            }
            // THE FONT'S DOCUMENT, which is the read-back's: the drain goes through the handle
            // that did the reading (#191).
            font.drained()?;
        }
        Ok(mapped)
    }

    fn page_keys(&self, read: &Self::Read, page: usize) -> Result<Vec<Vec<u8>>> {
        read.deadline.checkpoint(self.clock.as_ref())?;
        crate::pdfsyntax::dict::top_level_keys(&read.page(page)?.unparse())
    }
}
