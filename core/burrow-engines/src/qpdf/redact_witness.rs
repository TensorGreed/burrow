//! Reading a redacted document back, through a fresh document of the same engine.
//!
//! The engine half of [`crate::redact_verify`]; the policy is there and this is the reading. It
//! shares no state with the redaction that produced the bytes: a new document, a new page tree,
//! opened from the emitted bytes and nothing else. Written once over
//! [`crate::redact::graph`] (#191), so the web reads back through the same code as native.

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

/// The native witness, by the name its tests have always used.
#[cfg(test)]
pub(crate) type QpdfWitness = Witness<super::Qpdf>;

#[cfg(test)]
impl QpdfWitness {
    /// A witness spending the operation's ceilings and the operation's remaining time.
    pub(crate) const fn new(limits: Limits, clock: Arc<dyn Clock>, deadline: Deadline) -> Self {
        Witness::over(super::Qpdf, limits, clock, deadline)
    }
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
        super::redact_frame::of(&read.page(page)?)
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
            let font =
                super::redact_steps::pack(resources.font_in_scope(&glyph.source.font)?.object()?);
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

#[cfg(test)]
mod tests {
    use super::{ClearedWitness, QpdfWitness};
    use burrow_types::{Limits, ManualClock};
    use std::sync::Arc;

    /// A one-page document whose font maps two codes and whose page carries two keys.
    ///
    /// Hand-built so the numbers the assertions use are the fixture's own rather than a
    /// producer's.
    pub(super) fn document() -> Vec<u8> {
        let to_unicode = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
                          /CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
                          3 beginbfchar\n<0041> <0041>\n<0042> <0042>\n<0141> <0141>\n\
                          endbfchar\nendcmap\nend\nend\n";
        let content = "BT /F1 24 Tf 72 700 Td (A) Tj ET\n";
        let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 \
                 /LastChar 94 /Widths {widths} /ToUnicode 6 0 R \
                 /Encoding << /Type /Encoding /Differences [67 /Ccedilla] >> >>"
            ),
            format!(
                "<< /Length {} >>\nstream\n{to_unicode}endstream",
                to_unicode.len()
            ),
        ];
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn witness() -> QpdfWitness {
        let clock = Arc::new(ManualClock::new(0));
        let limits = Limits::default();
        let deadline = burrow_types::Deadline::start(clock.as_ref(), &limits);
        QpdfWitness::new(limits, clock, deadline)
    }

    #[test]
    fn the_witness_reports_the_codes_a_font_maps() {
        // NON-VACUITY FOR THE INSTRUMENT. Every assertion the verification makes is of the form
        // "this set is empty", so a witness reporting empty sets satisfies all of them over any
        // document. A mutation sweep planted exactly that and the end-to-end tests did not
        // notice, because they assert on the emitted bytes and not on what the check was told.
        //
        // The fixture maps 0x41 and 0x42 in `/ToUnicode` and names 0x43 in `/Differences`, so
        // the union is three codes and the count is knowable.
        let witness = witness();
        let bytes = document();
        let read = witness.open_output(&bytes).expect("opens");
        let mapped = witness.mapped_codes(&read, 0).expect("reads the fonts");

        assert_eq!(mapped.len(), 1, "one font on the page: {mapped:?}");
        let codes = mapped.values().next().expect("the font");
        assert_eq!(
            codes.len(),
            4,
            "0x41, 0x42 and 0x141 from /ToUnicode, 0x43 from /Differences: {codes:?}"
        );
        assert!(codes.contains(&0x41) && codes.contains(&0x42) && codes.contains(&0x43));
        // A CODE ABOVE 0x00FF, which is the whole two-byte half of the domain. A mutation
        // narrowing the scan to `0..=255` survived the suite because no fixture had one -- so
        // the Identity-H shape ADR 0029 calls legible-and-invisible was not covered by the
        // mapping read-back's own tests.
        assert!(
            codes.contains(&0x0141),
            "a two-byte code must be reported: {codes:?}"
        );
    }

    #[test]
    fn the_witness_reports_the_glyphs_a_page_draws() {
        // THE FIFTH TEST, and its absence was the sharpest finding of the review. Four direct
        // witness tests were written -- mapped codes, drawn codes, page keys, the page bound --
        // and not this one. A mutation making `glyphs_on` walk and then return an empty vector
        // survived the entire suite, which silently disables **check 1**: "no text-showing
        // operator remains whose glyphs fall inside the region", the strongest assertion in
        // ADR 0029 §6.
        //
        // The commit that introduced the other four says in its own message that a witness
        // returning empty sets satisfies every "this set is empty" assertion over any document.
        // It was right, and it did not finish applying the observation to itself.
        let witness = witness();
        let bytes = document();
        let read = witness.open_output(&bytes).expect("opens");
        let glyphs = witness.glyphs_on(&read, 0).expect("walks");

        assert_eq!(glyphs.len(), 1, "the fixture draws one glyph: {glyphs:?}");
        let glyph = &glyphs[0];
        assert_eq!(glyph.source.code, 0x41, "and it is `A`");
        assert!(
            (glyph.origin.0 - 72.0).abs() < 0.01 && (glyph.origin.1 - 700.0).abs() < 0.01,
            "placed where the content stream puts it, at {:?}",
            glyph.origin
        );
        assert!(
            glyph.conservative_box().right > glyph.conservative_box().left,
            "with a box that has extent -- a degenerate one would make every region test false"
        );
    }

    #[test]
    fn the_witness_reports_the_codes_a_page_draws() {
        // The other half of the comparison. The page draws `A` and nothing else, so the
        // difference against the three mapped codes is two -- which is what makes the mapping
        // check's subtraction a real question rather than an empty one.
        let witness = witness();
        let bytes = document();
        let read = witness.open_output(&bytes).expect("opens");
        let drawn = witness.drawn_codes(&read, 0).expect("walks");

        assert_eq!(drawn.len(), 1, "one font: {drawn:?}");
        let codes = drawn.values().next().expect("the font");
        assert_eq!(codes, &[0x41u32].into_iter().collect(), "only `A` is drawn");
    }

    #[test]
    fn the_witness_reports_the_pages_keys() {
        // Same reason: a witness reporting no keys passes the allowlist check over a page
        // carrying every carrier there is.
        let witness = witness();
        let bytes = document();
        let read = witness.open_output(&bytes).expect("opens");
        let keys = witness.page_keys(&read, 0).expect("reads the page");

        assert!(
            keys.len() >= 4,
            "the fixture's page has /Type, /Parent, /MediaBox, /Resources and /Contents: {keys:?}"
        );
        assert!(keys.iter().any(|key| key == b"Contents"), "{keys:?}");
        assert!(keys.iter().any(|key| key == b"MediaBox"), "{keys:?}");
    }

    #[test]
    fn a_page_past_the_end_is_a_rejected_output_rather_than_a_reach() {
        // `ObjectHandle::page` is unsafe on an index past the count, and the read-back is given
        // a page number by the caller. The bound is here and it is named as an OUTPUT problem:
        // a document burrow wrote that does not have the page it just redacted is not a
        // malformed input.
        let witness = witness();
        let bytes = document();
        let read = witness.open_output(&bytes).expect("opens");
        let error = witness
            .page_keys(&read, 7)
            .expect_err("page 7 does not exist");
        assert!(
            matches!(error, burrow_types::Error::OutputRejected(_)),
            "got: {error:?}"
        );
    }
}

#[cfg(test)]
mod ceiling_tests {
    use super::{ClearedWitness, QpdfWitness};
    use burrow_types::{Deadline, Limits, ManualClock};
    use std::sync::Arc;

    #[test]
    fn the_witness_opens_under_the_callers_ceilings_and_not_the_defaults() {
        // KILLS: `Limits::default()` inside `open_output`. The read-back holds a second parsed
        // document plus a copy of the output; a caller that set a ceiling for the operation and
        // had it silently raised for the verification has no ceiling.
        //
        // Stated rather than sized to a default: a fixture built to pass 512 MB would be a
        // memory experiment, and one sized to a default stops testing anything the day the
        // default moves.
        let bytes = super::tests::document();
        let clock = Arc::new(ManualClock::new(0));

        let mut tight = Limits::default();
        tight.max_input_bytes = 16;
        let deadline = Deadline::start(clock.as_ref(), &tight);
        let witness = QpdfWitness::new(
            tight,
            Arc::clone(&clock) as Arc<dyn burrow_types::Clock>,
            deadline,
        );
        let Err(error) = witness.open_output(&bytes) else {
            panic!("the document is larger than sixteen bytes and must be refused");
        };
        assert!(
            matches!(error, burrow_types::Error::LimitExceeded { .. }),
            "the caller's ceiling must apply to the read-back: {error:?}"
        );

        // THE NEAR-MISS. Without it this passes for a witness that refuses everything.
        let roomy = Limits::default();
        let deadline = Deadline::start(clock.as_ref(), &roomy);
        let witness = QpdfWitness::new(roomy, clock as Arc<dyn burrow_types::Clock>, deadline);
        assert!(
            witness.open_output(&bytes).is_ok(),
            "the default ceiling admits the same document"
        );
    }

    #[test]
    fn the_witness_spends_the_operations_remaining_time_and_not_a_fresh_budget() {
        // KILLS: taking `open_document`'s freshly started `Deadline`. `Deadline::start` resets
        // the origin AND the budget, so a witness spending that one gave the read-back a second
        // full `max_duration_ms` -- which `burrow_ops::verify::output` forbids in capitals and
        // ADR 0029's amendment described as shared when it was not.
        //
        // The clock is advanced past the budget before the witness is built, so a deadline that
        // is the operation's is already spent and one that is fresh is not.
        let clock = Arc::new(ManualClock::new(0));
        let mut limits = Limits::default();
        limits.max_duration_ms = 10;
        let deadline = Deadline::start(clock.as_ref(), &limits);
        clock.advance(1_000);

        let witness = QpdfWitness::new(limits, clock as Arc<dyn burrow_types::Clock>, deadline);
        let bytes = super::tests::document();
        let Ok(read) = witness.open_output(&bytes) else {
            panic!("opening does not checkpoint, so it succeeds");
        };
        let error = witness
            .page_keys(&read, 0)
            .expect_err("the operation's budget is long gone");
        assert!(
            matches!(error, burrow_types::Error::LimitExceeded { .. }),
            "the read-back must spend the operation's remaining time: {error:?}"
        );
    }

    /// A clock that holds still until told to move, then moves one millisecond per read.
    ///
    /// So the read-back's own entry checkpoint can be made to pass exactly, and only a read
    /// made INSIDE the walk can expire the budget.
    #[derive(Debug, Default)]
    struct HoldThenTick {
        now: std::sync::atomic::AtomicU64,
        step: std::sync::atomic::AtomicU64,
    }

    impl burrow_types::Clock for HoldThenTick {
        fn now_ms(&self) -> u64 {
            let step = self.step.load(std::sync::atomic::Ordering::SeqCst);
            self.now
                .fetch_add(step, std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[test]
    fn the_read_backs_glyph_walks_spend_the_operations_deadline() {
        // KILLS: a fresh or default deadline in `QpdfWitness::watch` (#175's code review swapped
        // it and nothing failed). The clock is parked AT the budget -- elapsed equals allowed,
        // which passes -- and then moves one tick per read, so the entry checkpoint in
        // `glyphs_on` passes and the walk's own post-lex read is the one that expires.
        for (name, ask) in [
            (
                "glyphs_on",
                (|witness: &QpdfWitness, read| witness.glyphs_on(read, 0).map(drop))
                    as fn(
                        &QpdfWitness,
                        &super::ReadBack<super::super::Document>,
                    ) -> burrow_types::Result<()>,
            ),
            ("drawn_codes", |witness, read| {
                witness.drawn_codes(read, 0).map(drop)
            }),
        ] {
            let clock = Arc::new(HoldThenTick::default());
            let mut limits = Limits::default();
            limits.max_duration_ms = 10;
            let deadline = Deadline::start(clock.as_ref(), &limits);
            let witness = QpdfWitness::new(
                limits,
                Arc::clone(&clock) as Arc<dyn burrow_types::Clock>,
                deadline,
            );
            let bytes = super::tests::document();
            let read = witness.open_output(&bytes).expect("opens");

            // THE NEAR-MISS FIRST: parked at the budget and not moving, the walk completes.
            clock.now.store(10, std::sync::atomic::Ordering::SeqCst);
            assert!(
                ask(&witness, &read).is_ok(),
                "{name}: at the budget, not past it"
            );

            clock.step.store(1, std::sync::atomic::Ordering::SeqCst);
            match ask(&witness, &read) {
                Err(burrow_types::Error::LimitExceeded { limit, .. }) => {
                    assert_eq!(limit, "max_duration_ms", "{name}");
                }
                other => panic!("{name}: the walk must read the operation's deadline: {other:?}"),
            }
        }
    }
}
