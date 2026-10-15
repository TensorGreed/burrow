//! Every page's display frame, read before a region exists (#136, slice S2).
//!
//! # What this answers, and for whom
//!
//! The redaction page asks a person to draw a region on a page, and a region is measured against
//! the page's **display frame**: its width and height in points after `/Rotate` and `/UserUnit`,
//! `/CropBox` falling back to `/MediaBox` -- exactly the frame [`Region`] converts against (ADR
//! 0029's 2026-09-22 amendment; [`PageFrame::displayed_size`]). So the page needs that frame per
//! page before anything is drawn, or it is drawing in a coordinate system it guessed.
//!
//! For each page this returns either the frame, or **the page-level rule that would refuse every
//! region on it** -- so a page can say "this page cannot be redacted, because…" before the person
//! draws, rather than after.
//!
//! # The same reader the redaction uses, not a second one
//!
//! Each page's answer is the redaction's own calls, in its order: `frame::check_inherited_media_box`
//! before the walk, then the walk's `/Resources` read (refused when the page tree supplies them),
//! `frame::of`, and the two checks [`Region::to_content_space`] makes of a frame (unit and
//! rotation, then the box). The document is opened through `OpensForRedaction::open_for_redaction`, the redaction's own
//! open, so a document the operation refuses at the open --
//! a ceiling, a password, a repaired input -- is refused here with the same error. Nothing here
//! reads a page's content: only the page dictionaries and the page tree above them.
//!
//! # What it does not say
//!
//! A frame is **not** a promise that a region on that page will redact. The operation refuses a
//! great deal more, and most of it depends on the region or on the content: what the region
//! reaches, a shared stream, an image under it, a form field anywhere in the document. Those
//! refusals arrive when the redaction runs. The document-level checks the operation makes after
//! its open and before its walk -- the reference pass (`[reference-to-nothing]`), the form-field
//! scan (`[acroform-field]`) and the sharing count -- are **not** run here, because each reads
//! every object the file references rather than only the page tree; the redaction still runs
//! them, so leaving them out moves a refusal later and never removes one. The walk's
//! optional-content refusal is page-level too and is not run either: it reads into the page's
//! resources rather than the page dictionary.
//!
//! **A page that draws nothing is no exception.** The redaction refuses a blank page with a bad
//! frame by the same error this reports for it -- measured for `/MediaBox [0 0 0 400]`,
//! `/UserUnit 2`, `/Rotate 90.5`, `/Rotate [90]` and an inherited `/Rotate`, and held by
//! `qpdf::page_frames_tests`' blank-page probe on both engines.
//!
//! [`Region`]: crate::pdfsyntax::region::Region
//! [`Region::to_content_space`]: crate::pdfsyntax::region::Region::to_content_space
//! [`PageFrame::displayed_size`]: crate::pdfsyntax::region::PageFrame::displayed_size
//! [`PageFrame::check_display_box`]: crate::pdfsyntax::region::PageFrame::check_display_box

use std::sync::Arc;

use burrow_types::{Error, Result};

use super::graph::{OpensForRedaction, PdfDocument, PdfObject};

/// A page-level rule that refuses every region on a page, as
/// [`PageFrameReader::page_frames`](crate::PageFrameReader::page_frames) reports it.
///
/// Each names the bracketed rule the refusal's message carries, word for word: the operation's
/// own refusal for the same page names the same rule, and a page can choose its sentence from
/// this without reading prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameRule {
    /// A box, `/Rotate` or `/UserUnit` in a shape the renderer reads differently, or a crop box
    /// that shares no area with the media box.
    PageFrameUnreadable,
    /// A `/UserUnit` other than 1.
    UserUnitNotOne,
    /// A `/CropBox`, `/Rotate` or `/Resources` taken from the page tree.
    PageAttributeInherited,
    /// A `/MediaBox` taken from the page tree that nothing confirmed is the one shown -- on the
    /// web, every such page, until the renderer reaches redaction (#206).
    MediaBoxUnverified,
    /// No `/MediaBox` anywhere up the page's tree.
    NoDisplayBox,
    /// A `/Rotate` that is not a whole number of degrees.
    RotateNotWhole,
    /// A `/Parent` chain deeper than the reader climbs.
    PageTreeDepth,
    /// A display box with no positive extent.
    DisplayBox,
    /// A whole `/Rotate` that is not a right angle (`/Rotate 45`): the region conversion refuses
    /// it, so every region on the page is refused.
    Rotate,
    /// A `/UserUnit` that is not positive and finite, refused by the region conversion. Not
    /// reachable through `frame.rs`, which refuses any unit but 1 first; read for the same reason
    /// the conversion checks it.
    UserUnit,
}

impl FrameRule {
    /// Every rule, in declaration order. What [`Self::of`] matches against.
    pub const ALL: [Self; 10] = [
        Self::PageFrameUnreadable,
        Self::UserUnitNotOne,
        Self::PageAttributeInherited,
        Self::MediaBoxUnverified,
        Self::NoDisplayBox,
        Self::RotateNotWhole,
        Self::PageTreeDepth,
        Self::DisplayBox,
        Self::Rotate,
        Self::UserUnit,
    ];

    /// The rule's name, as its refusal's message brackets it.
    #[must_use]
    pub const fn rule(self) -> &'static str {
        match self {
            Self::PageFrameUnreadable => "page-frame-unreadable",
            Self::UserUnitNotOne => "user-unit-not-one",
            Self::PageAttributeInherited => "page-attribute-inherited",
            Self::MediaBoxUnverified => "media-box-unverified",
            Self::NoDisplayBox => "no-display-box",
            Self::RotateNotWhole => "rotate-not-whole",
            Self::PageTreeDepth => "page-tree-depth",
            Self::DisplayBox => "display-box",
            Self::Rotate => "rotate",
            Self::UserUnit => "user-unit",
        }
    }

    /// The frame rule `error` refuses by, or `None` when it is not one.
    ///
    /// **Read from the message's first bracketed token**, because that is where `frame.rs` and
    /// `region.rs` write the rule, and it is matched exactly against [`Self::ALL`]. Only a
    /// `Malformed` or `Unsupported` is a page's refusal: a ceiling, an engine failure or an
    /// internal error is the whole operation's, and `None` here is what keeps it so.
    #[must_use]
    pub fn of(error: &Error) -> Option<Self> {
        let (Error::Malformed(message) | Error::Unsupported(message)) = error else {
            return None;
        };
        let open = message.find('[')?;
        let rest = message.get(open + 1..)?;
        let token = rest.get(..rest.find(']')?)?;
        Self::ALL.into_iter().find(|rule| rule.rule() == token)
    }
}

/// The longest side a frame can have: `frame.rs` refuses a box coordinate past 2^24 either way,
/// and `/UserUnit` other than 1, so a side spans at most twice that.
pub const MAX_FRAME_SIDE: f64 = 2.0 * super::frame::MAX_FRAME_MAGNITUDE;

/// Every page's reading, beside the page count the document was opened with.
///
/// The count is carried so a caller can hold the readings to it: one reading per page, no more and
/// no fewer, is what a page draws regions against, and a list one short would leave a page with
/// no frame -- or shift every later frame onto the wrong page.
#[derive(Debug)]
pub struct PageFrames {
    /// The page count the engine opened the document with.
    pub pages: u64,
    /// One reading per page, in page order.
    pub readings: Vec<PageFrameReading>,
}

/// One page's answer: its display frame, or the rule that refuses every region on it.
#[derive(Debug)]
#[non_exhaustive]
pub enum PageFrameReading {
    /// The displayed page's size in points, after `/Rotate` and `/UserUnit`: the frame a
    /// [`Region`](crate::pdfsyntax::region::Region) on this page is measured against. Positive and
    /// finite, and no larger than [`MAX_FRAME_SIDE`].
    Frame {
        /// Width of the displayed page, in points.
        width: f64,
        /// Height of the displayed page, in points.
        height: f64,
    },
    /// The page-level refusal every region on this page would meet.
    Refused {
        /// Which rule, as a value.
        rule: FrameRule,
        /// The refusal itself, exactly as the operation raises it for this page.
        error: Error,
    },
}

/// Every page's frame or page-level refusal, in page order, read as the redaction reads it.
///
/// `renderer_size` is [`OpensForRedaction::renderer_page_size`] for one page: the second reading an
/// inherited `/MediaBox` needs (see `frame::check_inherited_media_box`). It is a parameter so the
/// native engine can open its renderer once for the whole document rather than once per page.
///
/// # Errors
///
/// - Everything the redaction's open refuses with: every ceiling `options.limits` names before the
///   engine, the page count, the measured memory, a password, a repaired input.
/// - [`Error::LimitExceeded`] for `max_duration_ms`, checkpointed per page against a deadline
///   started at the open.
/// - `[engine-repaired-input]` if qpdf repaired the document while its pages were read: it reads
///   lazily, and a repair after the open's check changes what a page is.
/// - Any refusal that is not one of [`FrameRule`] -- an engine failure, or a ceiling the renderer
///   reached -- for the whole document rather than for one page: an error this cannot name is not
///   a page's to absorb.
pub(crate) fn page_frames<E: OpensForRedaction>(
    engine: &E,
    bytes: &[u8],
    options: &crate::OpenOptions<'_>,
    mut renderer_size: impl FnMut(usize) -> Result<Option<(f64, f64)>>,
) -> Result<PageFrames> {
    let clock = Arc::clone(&options.clock);
    let (document, deadline) = engine.open_for_redaction(bytes, options)?;
    let pages = document.page_count()?;
    let count = usize::try_from(pages)
        .map_err(|_| Error::Internal("a page count that does not fit in usize".to_owned()))?;
    let mut readings = Vec::new();
    readings
        .try_reserve_exact(count)
        .map_err(|_| Error::Internal("no room for one reading per page".to_owned()))?;
    for index in 0..count {
        deadline.checkpoint(clock.as_ref())?;
        let page = document.page(index)?;
        // IN THE REDACTION'S ORDER: the inherited `/MediaBox` check before the walk (which reads
        // the frame first), then the walk's own `/Resources` read -- refused when the tree supplies
        // them, on every region -- then the frame the region converts against.
        let reading = super::frame::check_inherited_media_box(&page, || renderer_size(index))
            .and_then(|()| super::resources::PageResources::of(&page).map(|_| ()))
            .and_then(|()| super::frame::of(&page))
            // AND THE CONVERSION'S OWN CHECKS, in its order (unit and rotation, then the box): a
            // frame the conversion refuses is no frame to draw a region on.
            .and_then(|frame| frame.check_unit_and_rotation().map(|()| frame))
            .and_then(|frame| frame.check_display_box().map(|()| frame));
        // WHAT THE ENGINE LATCHED WHILE THE PAGE WAS READ, before any of it is believed: a value
        // read past a latched error is not the document's, and that is the whole operation's
        // failure, not the page's. It matters only for a latch during the LAST page's reads: for
        // every other page the next `document.page()` drains the same latch. DEFENCE IN DEPTH, and
        // unwitnessed: no fixture latches an error while a frame is read, and the mutation that
        // deletes this drain survives the suite (measured).
        page.drained()?;
        readings.push(match reading {
            Ok(frame) => {
                let (width, height) = frame.displayed_size();
                PageFrameReading::Frame { width, height }
            }
            Err(error) => match FrameRule::of(&error) {
                Some(rule) => PageFrameReading::Refused { rule, error },
                None => return Err(error),
            },
        });
    }
    // A REPAIR WHILE THE PAGES WERE READ, refused as the redaction refuses one after its write
    // (#224): qpdf resolves the page tree lazily, and a repair there can change what a page says.
    if document.repaired() {
        return Err(super::repaired_by_the_engine());
    }
    Ok(PageFrames { pages, readings })
}

#[cfg(test)]
mod tests {
    use super::FrameRule;
    use burrow_types::Error;

    /// Each rule is read back from a message of its own, and refused from its near-misses: another
    /// variant, a token with a suffix, a rule outside the brackets, a bracket that does not close.
    #[test]
    fn each_rule_is_read_from_its_own_bracket_and_nothing_else() {
        let mut examined = 0;
        for rule in FrameRule::ALL {
            let name = rule.rule();
            let unsupported = Error::Unsupported(format!("pdf redaction [{name}]: why"));
            let malformed = Error::Malformed(format!("pdf region [{name}]: why"));
            assert_eq!(FrameRule::of(&unsupported), Some(rule), "{name}");
            assert_eq!(FrameRule::of(&malformed), Some(rule), "{name}");
            // NEAR-MISSES: the same text as another kind, a longer token, the name unbracketed,
            // an unclosed bracket, and the rule only in a SECOND bracket.
            for near in [
                Error::InvalidArgument(format!("pdf redaction [{name}]: why")),
                Error::Internal(format!("[{name}]")),
                Error::Unsupported(format!("pdf redaction [{name}-x]: why")),
                Error::Unsupported(format!("pdf redaction {name}: why")),
                Error::Unsupported(format!("pdf redaction [{name}")),
                Error::Unsupported(format!("pdf redaction [other]: [{name}]")),
            ] {
                assert_eq!(FrameRule::of(&near), None, "{near:?}");
            }
            examined += 1;
        }
        assert_eq!(examined, FrameRule::ALL.len());
        // A CEILING IS NEVER A PAGE'S REFUSAL.
        let limit = Error::LimitExceeded {
            limit: "max_duration_ms",
            stage: burrow_types::Stage::Deadline,
            requested: 2,
            allowed: 1,
        };
        assert_eq!(FrameRule::of(&limit), None);
    }

    /// The R8 spec's copy of the rule names (`apps/web/e2e/redaction-protocol.ts`, `FRAME_RULES`)
    /// is this enum's, exactly: a rule added here and not there would make R8 refuse a real reply,
    /// and one left there after it went here would admit a word nothing produces.
    #[test]
    fn the_r8_spec_lists_exactly_these_rules() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/web/e2e/redaction-protocol.ts"
        );
        let source = std::fs::read_to_string(path).unwrap();
        let start = source
            .find("const FRAME_RULES = [")
            .expect("the spec's FRAME_RULES list");
        let block = &source[start..];
        let block = &block[..block.find("];").expect("the list's end")];
        let listed: Vec<&str> = block
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|name| !name.is_empty())
            .collect();
        let ours: Vec<&str> = FrameRule::ALL.iter().map(|rule| rule.rule()).collect();
        assert_eq!(
            listed, ours,
            "the R8 spec's FRAME_RULES, against FrameRule::ALL"
        );
        // AND THE EMPTY NAME, which the spec admits for a page with a frame.
        assert!(
            block.contains("\"\","),
            "the spec must admit \"\" for a frame"
        );
    }

    /// Every rule name is distinct, so `of` cannot answer one rule with another.
    #[test]
    fn every_rule_name_is_distinct() {
        let names: std::collections::BTreeSet<&str> =
            FrameRule::ALL.iter().map(|rule| rule.rule()).collect();
        assert_eq!(names.len(), FrameRule::ALL.len());
    }
}
