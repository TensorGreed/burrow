// AWAITING ITS CONSUMER, and said so rather than silenced. Nothing calls this yet: #131's
// removal operation is the caller, and it is not assembled. `expect` rather than `allow`
// because it becomes an error the moment the walk is wired in, so this note cannot rot into a
// blanket exemption for genuinely dead code.
// Conditional because the tests DO use it: an unconditional `expect` is unfulfilled under
// `--all-targets`, which CI treats as an error. The shape says exactly what is true -- covered
// by tests, not yet called by production.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the sharing walk is complete and tested; #131's operation is its first caller"
    )
)]

//! How many places in a document draw each Form XObject.
//!
//! [ADR 0029](../../../../docs/adr/0029-what-redaction-does-and-what-it-refuses-to-do.md)'s
//! sharing amendment: editing a form that is drawn more than once removes its text **everywhere
//! it is drawn**, so a page nobody asked about silently loses content while the page the user
//! selected looks correctly redacted. §6's read-back cannot catch that — it asks about the page
//! it was given, and that page is clean.
//!
//! This module answers the question the refusal is built on, and nothing else. It does not
//! decide anything; `pdfsyntax::geometry::check_form_sharing` holds the rule, and it is scoped
//! to the glyphs actually being removed so that a letterhead the region avoids is not a bar.
//!
//! # Four things it is not allowed to get wrong
//!
//! **1. `/Resources` is inheritable.** A page with no `/Resources` of its own uses the nearest
//! ancestor's, exactly as `/Rotate` does — and `rotate.rs` has the same climb for the same
//! reason. Reading the page dictionary and concluding "this page draws nothing" is wrong for
//! every document that puts resources on a `/Pages` node, which is how a word processor
//! usually writes a shared letterhead. A form reached only through an inherited dictionary
//! would then be counted **once** instead of many times, and under-counting reads as
//! *unshared* — the direction that edits in place.
//!
//! **2. Identity, never name.** Two pages may both call a form `/Fm0` and mean different
//! objects; one page may reach the same object under two names. The count is keyed on object
//! number and generation. `core/CLAUDE.md` has the rule and `tools/check-handle-identity.py`
//! enforces that a raw handle is never compared.
//!
//! **3. Every graph that can draw a form**, not just page resources. A use counted in one place
//! and missed in another reads as unshared. The annotation case is the one a page-only scan
//! misses most easily, because an appearance stream is reached through `/Annots` rather than
//! through `/Resources` and is a form object in its own right.
//!
//! **4. The graph is a graph, and a hostile one is cyclic.** A form whose `/Resources` reaches
//! itself, or a pair that reach each other, would otherwise spin this walk forever inside a
//! call that holds the engine thread. Both are refused rather than truncated: a walk that
//! stopped early would report a count that is too low, which is again the unsafe direction.
//!
//! # Cost
//!
//! One pass over the object graph, which the redaction already pays to open the document.
//! Measured on generated documents in ADR 0029: 2.4 ms of bookkeeping against 110 ms of qpdf
//! object resolution at 5,000 pages, so the walk is not where the cost is — provided each
//! subtree is descended **once**. Every reference increments the count; only the first
//! descends. Without that a form referenced *n* times would cost *n* times its subtree.

use core::ffi::c_int;
use std::collections::{BTreeMap, BTreeSet};

use burrow_types::{Error, Result};

use super::Document;
use super::handle::ObjectHandle;
use super::name::Name;
use crate::codes::qpdf::object_type;

/// How deep the resource graph may nest before the walk refuses.
///
/// Forms nest through their own `/Resources`, and nothing in a file bounds that. Deeper than
/// any producer nests and shallow enough that the work is bounded well before the stack is.
/// Distinct from `pdfsyntax::geometry::MAX_FORM_DEPTH`, which bounds *drawing* recursion within
/// one page's content; this bounds the object graph.
pub(crate) const MAX_RESOURCE_DEPTH: u32 = 32;

/// The page-tree climb's ceiling, matching `rotate`'s for the same reason.
const MAX_PAGE_TREE_DEPTH: u32 = 64;

/// `/Resources`.
const RESOURCES: Name = Name::literal(b"/Resources\0");
/// `/Parent`.
const PARENT: Name = Name::literal(b"/Parent\0");
/// `/XObject`.
const XOBJECT: Name = Name::literal(b"/XObject\0");
/// `/Pattern`.
const PATTERN: Name = Name::literal(b"/Pattern\0");
/// `/Font`.
const FONT: Name = Name::literal(b"/Font\0");
/// `/CharProcs`.
const CHARPROCS: Name = Name::literal(b"/CharProcs\0");
/// `/Annots`.
const ANNOTS: Name = Name::literal(b"/Annots\0");
/// `/AP`.
const AP: Name = Name::literal(b"/AP\0");
/// `/N`.
const N: Name = Name::literal(b"/N\0");
/// `/D`.
const D: Name = Name::literal(b"/D\0");
/// `/R`.
const R: Name = Name::literal(b"/R\0");

/// A PDF object's identity: number and generation.
type ObjectId = (c_int, c_int);

/// How many places draw each Form XObject in a document.
#[derive(Debug, Default)]
pub(crate) struct FormUseCounts {
    counts: BTreeMap<ObjectId, usize>,
}

impl FormUseCounts {
    /// How many places draw the form with this identity.
    ///
    /// Zero for an object this walk never reached, which a caller should treat as "not a form
    /// this document draws" rather than as "unshared".
    pub(crate) fn uses(&self, object: ObjectId) -> usize {
        self.counts.get(&object).copied().unwrap_or(0)
    }

    /// Every form this walk found, for the tests that need to see the whole answer.
    #[cfg(test)]
    pub(crate) fn all(&self) -> &BTreeMap<ObjectId, usize> {
        &self.counts
    }
}

/// Count every place each Form XObject is drawn.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `resource-graph-cycle` or `resource-graph-depth` if the graph
/// is cyclic or nests past [`MAX_RESOURCE_DEPTH`]; [`Error::Malformed`] for a page tree that
/// does not terminate. Whatever the engine failed with.
pub(crate) fn count_form_uses(document: &Document) -> Result<FormUseCounts> {
    let mut walk = Walk {
        document,
        counts: BTreeMap::new(),
        descended: BTreeSet::new(),
        open: Vec::new(),
    };
    let pages = document.page_count()?;
    for index in 0..pages {
        let at = usize::try_from(index)
            .map_err(|_| Error::Internal("qpdf: a page index that is not an index".to_owned()))?;
        // SAFETY: `at` is below the page count just read from this document.
        let page = unsafe { ObjectHandle::page(document, at) };
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        walk.page(&page)?;
    }
    Ok(FormUseCounts {
        counts: walk.counts,
    })
}

/// The walk's state: what it has counted, what it has already descended, and what is open.
struct Walk<'a> {
    document: &'a Document,
    counts: BTreeMap<ObjectId, usize>,
    /// Subtrees already descended, so a form referenced *n* times costs its subtree once.
    descended: BTreeSet<ObjectId>,
    /// The objects on the current path, for cycle detection. **Not** the same set as
    /// `descended`: a form legitimately appearing twice is sharing, not a cycle, and a walk
    /// that confused the two would refuse the documents this exists to measure.
    open: Vec<ObjectId>,
}

impl Walk<'_> {
    /// One page: its inherited resources, then its annotations' appearance streams.
    fn page(&mut self, page: &ObjectHandle<'_>) -> Result<()> {
        let resources = self.inherited_resources(page)?;
        if let Some(resources) = resources {
            self.resources(&resources, 0)?;
        }
        self.annotations(page)?;
        Ok(())
    }

    /// `/Resources`, climbing `/Parent` when the page declares none.
    ///
    /// The same shape as `rotate::effective_rotation`, and for the same reason: a page that
    /// inherits its resources draws whatever the ancestor names, and a walk that stopped at the
    /// page dictionary would count those forms once instead of once per page.
    fn inherited_resources<'h>(&self, page: &ObjectHandle<'h>) -> Result<Option<ObjectHandle<'h>>> {
        let direct = self.dictionary_key(page, &RESOURCES)?;
        if direct.is_some() {
            return Ok(direct);
        }
        let mut current = page.key(&PARENT);
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        for _ in 0..MAX_PAGE_TREE_DEPTH {
            let code = current.type_code();
            if let Some(error) = self.document.take_error() {
                return Err(error);
            }
            match code {
                object_type::DICTIONARY => {}
                // The top of the tree, or a `/Parent` that is not a node. Either way there is
                // nothing above to inherit from, which is not an error.
                _ => return Ok(None),
            }
            if let Some(found) = self.dictionary_key(&current, &RESOURCES)? {
                return Ok(Some(found));
            }
            let next = current.key(&PARENT);
            if let Some(error) = self.document.take_error() {
                return Err(error);
            }
            current = next;
        }
        Err(Error::Malformed(
            "qpdf sharing [page-tree-depth]: a /Parent chain deeper than burrow will climb, \
             which a cycle in the page tree also produces"
                .to_owned(),
        ))
    }

    /// One resource dictionary: its forms, patterns and Type 3 fonts.
    fn resources(&mut self, resources: &ObjectHandle<'_>, depth: u32) -> Result<()> {
        if depth > MAX_RESOURCE_DEPTH {
            return Err(Error::Unsupported(
                "qpdf sharing [resource-graph-depth]: a resource graph nested deeper than \
                 burrow will walk"
                    .to_owned(),
            ));
        }
        for (category, is_form) in [(&XOBJECT, true), (&PATTERN, false)] {
            let Some(dictionary) = self.dictionary_key(resources, category)? else {
                continue;
            };
            for name in self.keys_of(&dictionary)? {
                let entry = dictionary.key(&name);
                if let Some(error) = self.document.take_error() {
                    return Err(error);
                }
                self.drawable(&entry, is_form, depth)?;
            }
        }
        self.type_three_fonts(resources, depth)?;
        Ok(())
    }

    /// A Form XObject, or a tiling pattern, reached from a resource dictionary.
    fn drawable(&mut self, entry: &ObjectHandle<'_>, count: bool, depth: u32) -> Result<()> {
        let code = entry.type_code();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        if code != object_type::STREAM {
            // An image `/XObject`, a shading pattern, or a key that is not an object at all.
            // None of them draws glyphs, so none of them is a use of a form.
            return Ok(());
        }
        let object = entry.object()?;
        if count {
            *self.counts.entry(object).or_insert(0) += 1;
        }
        self.descend(entry, object, depth)
    }

    /// A stream's own `/Resources`, once per object however many times it is referenced.
    fn descend(&mut self, stream: &ObjectHandle<'_>, object: ObjectId, depth: u32) -> Result<()> {
        // A CYCLE IS A REFUSAL, not a stop. Keyed on the OPEN PATH rather than on everything
        // seen: a form drawn from two pages is sharing, which is the thing being measured, and
        // a walk that called that a cycle would refuse every document it exists for.
        if self.open.contains(&object) {
            return Err(Error::Unsupported(
                "qpdf sharing [resource-graph-cycle]: a form's resources reach the form itself, \
                 directly or through another"
                    .to_owned(),
            ));
        }
        if !self.descended.insert(object) {
            return Ok(());
        }
        let dictionary = stream.stream_dict();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        let Some(resources) = self.dictionary_key(&dictionary, &RESOURCES)? else {
            return Ok(());
        };
        self.open.push(object);
        let result = self.resources(&resources, depth + 1);
        self.open.pop();
        result
    }

    /// `/Font` entries that are Type 3, whose `/CharProcs` draw like any other stream.
    fn type_three_fonts(&mut self, resources: &ObjectHandle<'_>, depth: u32) -> Result<()> {
        let Some(fonts) = self.dictionary_key(resources, &FONT)? else {
            return Ok(());
        };
        for name in self.keys_of(&fonts)? {
            let font = fonts.key(&name);
            if let Some(error) = self.document.take_error() {
                return Err(error);
            }
            let Some(procs) = self.dictionary_key(&font, &CHARPROCS)? else {
                continue;
            };
            // The font's own `/Resources` is what its procedures draw against.
            if let Some(inner) = self.dictionary_key(&font, &RESOURCES)? {
                self.resources(&inner, depth + 1)?;
            }
            for proc_name in self.keys_of(&procs)? {
                let procedure = procs.key(&proc_name);
                if let Some(error) = self.document.take_error() {
                    return Err(error);
                }
                // A glyph procedure is not itself a form, so it is descended but not counted.
                self.drawable(&procedure, false, depth)?;
            }
        }
        Ok(())
    }

    /// `/Annots` → `/AP` → `/N`, `/D`, `/R`: appearance streams are forms with resources.
    fn annotations(&mut self, page: &ObjectHandle<'_>) -> Result<()> {
        let annots = page.key(&ANNOTS);
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        let code = annots.type_code();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        if code != object_type::ARRAY {
            return Ok(());
        }
        let count = annots.array_len();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        for at in 0..count {
            let annotation = annots.array_item(at);
            if let Some(error) = self.document.take_error() {
                return Err(error);
            }
            let Some(appearances) = self.dictionary_key(&annotation, &AP)? else {
                continue;
            };
            for state in [&N, &D, &R] {
                let appearance = appearances.key(state);
                if let Some(error) = self.document.take_error() {
                    return Err(error);
                }
                // An appearance may be a stream, or a dictionary of states each of which is.
                let kind = appearance.type_code();
                if let Some(error) = self.document.take_error() {
                    return Err(error);
                }
                if kind == object_type::DICTIONARY {
                    for name in self.keys_of(&appearance)? {
                        let one = appearance.key(&name);
                        if let Some(error) = self.document.take_error() {
                            return Err(error);
                        }
                        self.drawable(&one, true, 0)?;
                    }
                } else {
                    self.drawable(&appearance, true, 0)?;
                }
            }
        }
        Ok(())
    }

    /// A dictionary-valued key, or `None` when it is absent or is not a dictionary.
    fn dictionary_key<'h>(
        &self,
        object: &ObjectHandle<'h>,
        key: &Name,
    ) -> Result<Option<ObjectHandle<'h>>> {
        let value = object.key(key);
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        let code = value.type_code();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        Ok((code == object_type::DICTIONARY).then_some(value))
    }

    /// A dictionary's keys, as [`Name`]s qpdf will accept.
    ///
    /// Through `unparse` and `pdfsyntax::dict`, because `qpdf_oh_get_dict_keys` is neither
    /// trapped nor accepted — the same route `web/extract.rs` takes for the same reason.
    ///
    /// `top_level_keys` strips the leading `/` and qpdf requires it; [`Name::from_stripped`]
    /// is the one place that conversion happens, and it refuses a key containing a NUL rather
    /// than letting the C string end early and act on a shorter key.
    fn keys_of(&self, dictionary: &ObjectHandle<'_>) -> Result<Vec<Name>> {
        let unparsed = dictionary.unparse();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        crate::pdfsyntax::dict::top_level_keys(&unparsed)?
            .iter()
            .map(|key| Name::from_stripped(key))
            .collect()
    }
}
