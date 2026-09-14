//! The qpdf implementation of [`PageRotator`].
//!
//! # Three things this operation is not allowed to get wrong
//!
//! **1. `/Rotate` is inheritable.** A page with no `/Rotate` of its own displays at whatever
//! the nearest ancestor in the page tree says. Reading the page dictionary and concluding
//! "no rotation" is wrong for every document that sets it on a `/Pages` node — which is how
//! a scanner that rotates a whole batch usually writes it. So [`effective_rotation`] walks
//! `/Parent` upward.
//!
//! **2. The write goes on the page, never on the ancestor it was read from.** That ancestor
//! may be the parent of every page in the document; setting `/Rotate` there to rotate one page
//! rotates all of them, and the operation reports success. `rotate` writes to the page
//! dictionary and nowhere else, and `rotating_one_page_leaves_every_other_page_alone_including_ones_that_inherit`
//! measures it on a document whose pages inherit.
//!
//! **3. A trapped call can still return a wrong answer.** `qpdf_oh_get_int_value` on a name
//! object returns 0; `qpdf_oh_get_key` on a non-dictionary returns a null object. Neither
//! raises, so `trap_errors` never sees anything. Every read here asks
//! [`ObjectHandle::type_code`] first and maps an unexpected type to `Malformed` — a document
//! whose `/Rotate` is `/Ninety` is malformed, and refusing it is better than silently treating
//! it as 0 and emitting a page turned the wrong way.
//!
//! # Handles are released, and the release is measured
//!
//! Every handle goes through [`ObjectHandle`], which releases on drop. `handle.rs` explains
//! why that matters: the cache only grows, and a per-page ancestor walk is the shape that
//! fills it. The test suite asserts the live count returns to its baseline after rotating a
//! document with a deep page tree, so the claim is a measurement rather than a type argument.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Rotation, Stage};

use super::handle::ObjectHandle;
use super::{Document, Qpdf};
use crate::codes::qpdf::object_type;
use crate::{OpenOptions, PageRotator};

/// `/Rotate`, as a NUL-terminated C string.
///
/// A literal in this crate, never anything derived from a document: qpdf takes `char const*`
/// and a key built from file content would be a way for a file to name its own keys.
const ROTATE_KEY: &[u8] = b"/Rotate\0";

/// `/Parent`, as a NUL-terminated C string.
const PARENT_KEY: &[u8] = b"/Parent\0";

/// How far up the page tree the inheritance walk will go before refusing.
///
/// The page tree is a tree, so a legitimate document cannot need more — qpdf's own parser
/// nesting ceiling is 64 (`codes::qpdf::policy::PARSER_MAX_NESTING`) and this matches it. A
/// document that exceeds it is either absurdly nested or has a `/Parent` cycle, and **a cycle
/// is the case this really guards**: `/Parent` pointing at a descendant is a two-line edit to
/// any PDF and would otherwise spin this loop forever, inside a call that holds a lock on
/// nothing and reports to nobody. All input is hostile.
const MAX_PAGE_TREE_DEPTH: u32 = 64;

/// A document held open for a rotation.
pub struct QpdfRotatable {
    document: Document,
    pages: u64,
    /// The resident set before the document was opened. `max_memory_bytes` **detects rather
    /// than bounds** (ADR 0007), and detecting it needs a before.
    rss_before: Option<u64>,
    /// The ceilings the document was opened under, so `rotate` applies the same ones a later
    /// caller cannot loosen.
    limits: Limits,
}

impl PageRotator for Qpdf {
    type Source = QpdfRotatable;

    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        // Every ceiling, in one place, shared with the other operations that open a document
        // this way -- see `open_document` for why this is not written out here.
        let (document, pages, rss_before) = super::open_document(bytes, options)?;
        Ok(QpdfRotatable {
            document,
            pages,
            rss_before,
            limits: options.limits,
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(source.pages)
    }

    fn effective_rotation(&self, source: &Self::Source, index: u64) -> Result<Rotation> {
        let page = page_handle(&source.document, index, source.pages)?;
        effective_rotation(&source.document, &page)
    }

    fn rotate(
        &self,
        source: &Self::Source,
        pages: &[u64],
        rotation: Rotation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        // `options` carries the CLOCK; the CEILINGS come from `source.limits` — the ceiling
        // that counts is the one the document was opened under, and a caller passing laxer
        // options to a later call must not be able to raise it after the fact. `extract`
        // records the same reasoning, and the two checks disagreeing there was an
        // inconsistency rather than a design.
        //
        // An earlier version discarded `options` entirely, which discarded the clock with it:
        // `max_duration_ms` was checked twice in `burrow_ops::rotate`, around this whole call,
        // and nowhere inside it. On a default `max_pages` of 10,000 with a 63-deep page tree
        // that is roughly 1.9 M FFI calls plus a full rewrite between two checkpoints, while
        // `Limits`' own rustdoc promises a check "at page boundaries". Found by security
        // review.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &source.limits);

        if pages.is_empty() {
            return Err(Error::InvalidArgument("no pages to rotate".to_owned()));
        }
        let count = u64::try_from(pages.len())
            .map_err(|_| Error::Internal("page count does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::PageCount,
            "max_pages",
            count,
            source.limits.max_pages,
        )?;

        // A page named twice would be rotated twice, which is a silently different document
        // from the one the caller asked for. Refused rather than deduplicated: the caller
        // meant something, and guessing which is worse than saying so.
        let mut seen = std::collections::BTreeSet::new();
        for &index in pages {
            if index >= source.pages {
                return Err(Error::InvalidArgument(
                    "page is not in the document".to_owned(),
                ));
            }
            if !seen.insert(index) {
                return Err(Error::InvalidArgument(
                    "the same page is named more than once".to_owned(),
                ));
            }
        }

        for &index in pages {
            // THE PAGE BOUNDARY `Limits` PROMISES. One check per page: the walk below is up
            // to `MAX_PAGE_TREE_DEPTH` ancestors and several engine calls, and no engine here
            // offers a timeout or an abort hook (ADR 0007), so a page is the finest grain
            // available.
            deadline.checkpoint(clock.as_ref())?;

            let page = page_handle(&source.document, index, source.pages)?;

            // READ THE EFFECTIVE VALUE, not the page's own. A page inheriting 90 that is
            // turned another 90 must end at 180; reading only its own dictionary would see
            // nothing, write 90, and quietly *undo* an inherited quarter turn.
            let current = effective_rotation(&source.document, &page)?;
            let wanted = rotation.after(current);

            let value = ObjectHandle::new_integer(&source.document, wanted.degrees());
            if let Some(error) = source.document.take_error() {
                return Err(error);
            }

            // ON THE PAGE. Never on the ancestor `current` may have come from — that node can
            // be the parent of every page in the document.
            page.replace_key(ROTATE_KEY.as_ptr().cast(), &value);
            if let Some(error) = source.document.take_error() {
                return Err(error);
            }
        }

        // The document is written back out as itself: rotate changes an attribute in place,
        // so there is no second document to copy pages into. `write_out` takes the source
        // twice to say that, rather than leaving its "the source must outlive the write" rule
        // looking inapplicable here.
        // And once more before the write, which is the single largest piece of work here.
        deadline.checkpoint(clock.as_ref())?;
        let output = super::extract::write_out(&source.document, &source.document)?;

        // `max_memory_bytes` DETECTS rather than bounds (ADR 0007). Checked after the write,
        // where the output buffer is at its largest.
        crate::estimate::check_measured_memory(
            source.rss_before,
            crate::rss::resident_bytes(),
            &source.limits,
        )?;

        Ok(output)
    }
}

/// The handle for page `index`, bounds-checked first.
fn page_handle(document: &Document, index: u64, pages: u64) -> Result<ObjectHandle<'_>> {
    if index >= pages {
        return Err(Error::InvalidArgument(
            "page is not in the document".to_owned(),
        ));
    }
    let n = usize::try_from(index)
        .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;

    // SAFETY: `n` is below the document's page count, checked above.
    let page = unsafe { ObjectHandle::page(document, n) };
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    Ok(page)
}

/// The rotation `page` displays at, following `/Rotate` up the page tree.
///
/// Returns [`Rotation::None`] when no ancestor carries the key, which is what a PDF with no
/// `/Rotate` anywhere means.
pub(super) fn effective_rotation<'a>(
    document: &'a Document,
    page: &ObjectHandle<'a>,
) -> Result<Rotation> {
    // The walk owns exactly one handle at a time: `node` is replaced by its parent, and the
    // previous one is dropped — and therefore released — at that moment. A version of this
    // that collected ancestors into a `Vec` first would hold one handle per level, which is
    // the shape `handle.rs` is about.
    let mut node = page.key(document, ROTATE_KEY.as_ptr().cast());
    // The page's own value first, then ancestors. `node` above is the *value*; the walk below
    // moves over page-tree *nodes*, so they are kept apart deliberately.
    let own = rotation_of(document, &node)?;
    if let Some(rotation) = own {
        return Ok(rotation);
    }
    drop(node);

    let mut current = page.key(document, PARENT_KEY.as_ptr().cast());
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    for _ in 0..MAX_PAGE_TREE_DEPTH {
        let parent_type = current.type_code();
        // CHECKED, like every other engine call in this file. This one was not, and the gap
        // was real: `qpdf_oh_get_type_code` resolves the object, `trap_oh_errors` catches a
        // throw and returns `ot_uninitialized` (0) as its fallback — which is neither
        // `DICTIONARY` nor `NULL`, so the walk read it as "top of the tree" and returned
        // `Ok(Rotation::None)` with an error still sitting in qpdf's slot. A genuine engine
        // failure reported as "no rotation". Found by security review, which could not
        // demonstrate reachability on qpdf 12.4.1 — every malformed `/Parent` it tried
        // resolved to null with a warning rather than throwing.
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        if parent_type == object_type::NULL {
            // The top of a well-formed tree: `/Parent` absent reads as a null object.
            return Ok(Rotation::None);
        }
        if parent_type != object_type::DICTIONARY {
            // A `/Parent` pointing at something that cannot be a page-tree node. Malformed
            // rather than folded in with the line above: "the walk ended" and "the walk hit
            // something that should not be there" are different facts, and reporting the
            // second as the first is how a refusal turns into a confident wrong answer.
            return Err(Error::Malformed(
                "a page tree node's /Parent is not a dictionary".to_owned(),
            ));
        }

        node = current.key(document, ROTATE_KEY.as_ptr().cast());
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        if let Some(rotation) = rotation_of(document, &node)? {
            return Ok(rotation);
        }
        drop(node);

        let parent = current.key(document, PARENT_KEY.as_ptr().cast());
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        current = parent;
    }

    // Depth exhausted. A page tree this deep is either absurd or a `/Parent` cycle; either
    // way the answer is a refusal rather than a guess or a spin.
    Err(Error::Malformed(
        "the page tree is deeper than this engine will walk".to_owned(),
    ))
}

/// `Some(rotation)` if `value` is a usable `/Rotate`, `None` if the key was absent.
///
/// **The type is asserted before the value is read.** `qpdf_oh_get_int_value` returns 0 for a
/// name, a string or a dictionary, and returns it without raising — so trapping does not help
/// and a `/Rotate /Ninety` would read as "no rotation" and emit a page turned the wrong way.
/// An unexpected type is [`Error::Malformed`], which is what it is.
fn rotation_of(document: &Document, value: &ObjectHandle<'_>) -> Result<Option<Rotation>> {
    let type_code = value.type_code();
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    match type_code {
        object_type::NULL => Ok(None),
        object_type::INTEGER => {
            let degrees = value.integer_value();
            if let Some(error) = document.take_error() {
                return Err(error);
            }
            // A `/Rotate` that is an integer but not a multiple of 90 is out of spec. It is
            // mapped to `Malformed` rather than rounded: rounding invents a rotation the file
            // never asked for, and the caller cannot tell it happened.
            Rotation::from_degrees(degrees)
                .map(Some)
                .map_err(|_| Error::Malformed("/Rotate is not a multiple of 90".to_owned()))
        }
        // Everything else — a name, a string, an array, a stream, an unresolvable reference.
        _ => Err(Error::Malformed("/Rotate is not an integer".to_owned())),
    }
}

/// One page handle, for the test that proves the live-handle count can move.
///
/// A counter that never changes reports "nothing leaked" for every input, including a leak.
/// `rotate_tests.rs` holds a handle through this and asserts the count notices, which is what
/// makes the baseline assertion beside it a measurement rather than a formality.
#[cfg(test)]
pub(super) fn page_handle_for_test(
    source: &QpdfRotatable,
    index: u64,
) -> Result<super::handle::ObjectHandle<'_>> {
    page_handle(&source.document, index, source.pages)
}
