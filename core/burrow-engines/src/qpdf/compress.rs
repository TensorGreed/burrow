//! The qpdf implementation of [`DocumentCompressor`].
//!
//! # One setter, and the module is nearly empty because of it
//!
//! [Spike 0005](../../../../docs/spikes/0005-what-qpdf-alone-compresses.md) measured what qpdf
//! alone delivers, and the answer changed what this module had to be. Of the five compression
//! levers qpdf's C API exposes, **four are already the writer's own defaults**
//! (`QPDFWriter_private.hh:296-315`): streams are compressed, unreferenced objects are dropped,
//! the decode level is `generalized`, and output is not linearized. `merge`, `rotate`,
//! `reorder` and `split` have been emitting all four since they shipped.
//!
//! So compression here is `qpdf_set_object_stream_mode(qpdf_o_generate)` and nothing else. The
//! module is short because the finding was that it should be, not because something was left
//! out — and the four defaults are deliberately **not** re-stated as configuration, because a
//! default spelled out as a setting is an invitation to tune what every other operation
//! already emits.
//!
//! # In place, not built
//!
//! Like `rotate` and `reorder`, and for the same reason: every input page appears in the
//! output, so nothing is excluded and
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's subsetting rule
//! has nothing to bite on. Building into a blank destination — the route `extract` must take —
//! would drop the outline, the attachments and the `/AcroForm`, which is the correct trade
//! when carrying them would be a leak and **pure loss** when the operation's entire promise is
//! that nothing changes but the encoding.
//!
//! Concretely: this module never copies a page. It hands the document it opened to the writer.
//!
//! # Lossless, and what makes that checkable rather than asserted
//!
//! No image is re-encoded, no font is subsetted, no content stream's meaning changes — because
//! nothing here touches any of them. qpdf offers no image or font handling at any setting, and
//! the one lever that is set governs how objects are *stored*, not what they contain.
//!
//! That is what lets `compress` claim the ROADMAP's *text remains extractable* invariant by
//! construction rather than by testing for it afterwards.
//!
//! # The output can be larger, and this module returns it anyway
//!
//! Measured at 3.4% of qpdf's own corpus, concentrated in small files but **not confined to
//! them**: an object stream carries
//! a fixed overhead — its own dictionary, its offset table, its cross-reference stream — so on
//! a document with little structure to pack it costs more than it saves.
//!
//! Deciding not to hand a larger output to the caller is `burrow_ops::compress`'s job. Two
//! reasons it is not this module's: returning the caller's own bytes is a policy about the
//! request rather than a fact about the document, and an engine that did it silently would
//! make [ADR 0022](../../../../docs/adr/0022-every-operation-verifies-its-own-output.md)'s
//! read-back verify bytes burrow did not produce, with nothing saying so.

use std::sync::Arc;

use burrow_types::{Deadline, Limits, Result};

use super::{Document, Qpdf};
use crate::{DocumentCompressor, OpenOptions};

/// A document held open for compression.
pub struct QpdfCompressible {
    document: Document,
    pages: u64,
    /// The resident set before the document was opened. `max_memory_bytes` **detects rather
    /// than bounds** (ADR 0007), and detecting it needs a before.
    rss_before: Option<u64>,
    /// The ceilings the document was opened under, so a later caller cannot loosen them.
    limits: Limits,
}

impl DocumentCompressor for Qpdf {
    type Source = QpdfCompressible;

    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        // Every ceiling, in one place, shared with the other operations that open a document
        // this way -- see `open_document` for why this is not written out here.
        let (document, pages, rss_before, _deadline) = super::open_document(bytes, options)?;
        Ok(QpdfCompressible {
            document,
            pages,
            rss_before,
            limits: options.limits,
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(source.pages)
    }

    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // THE SAME WALK `rotate` AND `reorder` DO. Reusing their helpers rather than repeating
        // the `/Parent` climb: the depth ceiling and the type assertion live there, and a
        // second copy of a walk over hostile input is a second place to get them wrong.
        // ONE SWEEP IMPLEMENTATION -- see `rotations_of`. This was a fourth copy of the walk
        // when it was written, which is what code review caught.
        //
        // THE CALLER'S DEADLINE, NOT A NEW ONE: `Deadline::start` resets the origin AND the
        // budget, so a sweep that started its own would hand the operation another full
        // `max_duration_ms`. ADR 0022 records that taking three attempts.
        super::rotations_of(&source.document, source.pages, options, deadline)
    }

    fn compress(&self, source: &Self::Source, options: &OpenOptions<'_>) -> Result<Vec<u8>> {
        // `options` carries the CLOCK; the CEILINGS come from `source.limits`, for the reason
        // `rotate` and `reorder` record: the ceiling that counts is the one the document was
        // opened under, and a caller passing laxer options to a later call must not be able to
        // raise it after the bytes are already in memory.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &source.limits);
        deadline.checkpoint(clock.as_ref())?;

        // THERE IS NO `max_pages` CHECK HERE, and that is deliberate rather than an omission.
        // `open_document` already refused `source.pages > max_pages` under these same
        // `Limits`, and compression takes no page selection -- there is no caller-chosen count
        // that could exceed the document's own. `rotate` keeps its equivalent because a
        // rotation's page list IS caller-chosen and can be longer than the document;
        // `reorder` drops its for the same reason this does, and records the same measurement.
        //
        // A ceiling that cannot fire reads as coverage, which `CLAUDE.md` calls worse than no
        // check at all.

        // THE WHOLE OPERATION. One call, one lever.
        //
        // The same document is passed as both arguments because this edits nothing and copies
        // nothing: the document that must outlive the write IS the one being written, which is
        // the in-place case `write_out`'s second parameter documents.
        let output = super::extract::write_out(
            &source.document,
            &source.document,
            super::extract::ObjectStreams::Generate,
        )?;
        deadline.checkpoint(clock.as_ref())?;

        // `max_memory_bytes` DETECTS rather than bounds (ADR 0007). Checked after the write,
        // where the output buffer is at its largest -- and object stream generation is the one
        // operation here that builds a second encoded copy of most of the document before
        // emitting it, so this is the path where a detection is most likely to be the one that
        // fires.
        crate::estimate::check_measured_memory(
            source.rss_before,
            crate::rss::resident_bytes(),
            &source.limits,
        )?;

        Ok(output)
    }
}
