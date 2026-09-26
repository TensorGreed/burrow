//! The read-back witness, against the real engine.
//!
//! Moved out of the witness's own file when the witness moved to `crate::redact` (#191): the
//! witness is written once for both engines, and these tests open documents with the native one.
//! Their bodies are as they were.

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Limits};

use crate::redact::witness::{ReadBack, Witness};
use crate::redact_verify::ClearedWitness;

/// The native witness, by the name its tests have always used.
pub(crate) type QpdfWitness = Witness<super::Qpdf>;

impl QpdfWitness {
    /// A witness spending the operation's ceilings and the operation's remaining time.
    pub(crate) const fn new(limits: Limits, clock: Arc<dyn Clock>, deadline: Deadline) -> Self {
        Witness::over(super::Qpdf, limits, clock, deadline)
    }
}

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
