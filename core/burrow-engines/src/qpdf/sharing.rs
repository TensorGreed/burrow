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

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Result};

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

/// How many resource dictionaries one walk will read in total.
///
/// # Depth bounds nothing on its own
///
/// [`MAX_RESOURCE_DEPTH`] caps the *path*, and a review measured what that leaves open: a
/// **branching** ladder of Type 3 fonts, each rung's `/Resources` naming two fonts on the next,
/// is `2^depth` -- 10.12 s at depth 22 from 6 kB, `4x` per two rungs, extrapolating to roughly
/// **2.9 hours from about 8.6 kB**, returning `Ok`. A cyclic version refuses in 209 us, so the
/// attack needs a *terminating* ladder, which is why the linear depth fixture never saw it.
///
/// Stream subtrees were already descended once per object; dictionary recursion -- a font's own
/// `/Resources` -- had no memo at all. Both are memoised now, and this is the backstop for a
/// shape neither memo covers.
const MAX_RESOURCE_DICTIONARIES: usize = 4096;

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
///
/// # Transitive, and a review found out why that matters
///
/// A first version counted each *reference* and descended each subtree once. That is right for
/// cost and wrong for multiplicity: a form `B` referenced once by a form `A` that is drawn on
/// two pages is reached twice and counted **once**.
///
/// Measured on exactly that document: `A` counted 2, `B` counted 1. `B` then reads as
/// **unshared**, takes the sanctioned in-place edit path, and removing text from it removes it
/// from a page nobody selected -- the damage the sharing rule exists to prevent, and the thing
/// §6's read-back cannot see. None of the seven fixtures was two levels deep.
///
/// So references are collected as a graph and the counts are propagated down it. Cycles are
/// refused during collection, so the graph is a DAG and the propagation terminates.
#[derive(Debug, Default)]
pub(crate) struct FormUseCounts {
    counts: BTreeMap<ObjectId, usize>,
    /// Which pages use each font object.
    ///
    /// # Why pages and not a count
    ///
    /// A form's question is "is it drawn more than once"; a font's is different. Font surgery
    /// removes `/Widths`, `/ToUnicode` and `/Differences` entries for codes nothing draws any
    /// more, and a font is shared by nearly every multi-page document — so refusing on a shared
    /// font would refuse nearly every multi-page document, and editing one in place reflows
    /// text on pages nobody redacted. That second outcome is **corruption, not leakage**, and
    /// it is worse: the other page still says something, and it says something different.
    ///
    /// So the rule is neither refuse-always nor edit-always: **cut a font only when every page
    /// that uses it is being redacted in this operation.** That needs the set, not a total.
    fonts: BTreeMap<ObjectId, std::collections::BTreeSet<usize>>,
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

    /// The fonts whose every using page is in `redacted`, so cutting them changes nothing else.
    ///
    /// # The rule, and why it is this one
    ///
    /// Editing a font used by a page outside the operation reflows that page's text — the
    /// glyphs are still there, at different widths, so it renders and says something slightly
    /// different. Corruption rather than leakage, and invisible to §6's read-back because that
    /// asks about the page it was given.
    ///
    /// Refusing whenever a font is shared would refuse nearly every multi-page document, since
    /// one font per document is the ordinary case rather than the exotic one. Copy-on-write is
    /// the answer that serves both, and it hits the same untrapped-verb wall as forms —
    /// `qpdf_oh_new_dictionary` is not on the trapped list either. ADR 0029 records both under
    /// one condition for revisiting.
    ///
    /// So: cut when the operation already covers every page that would be affected, and
    /// otherwise leave the font intact and disclose it (§7).
    pub(crate) fn fonts_wholly_within(
        &self,
        redacted: &std::collections::BTreeSet<usize>,
    ) -> std::collections::BTreeSet<ObjectId> {
        self.fonts
            .iter()
            .filter(|(_, pages)| pages.is_subset(redacted))
            .map(|(font, _)| *font)
            .collect()
    }

    /// Which pages use each font.
    pub(crate) const fn font_pages_map(
        &self,
    ) -> &BTreeMap<ObjectId, std::collections::BTreeSet<usize>> {
        &self.fonts
    }

    /// Which pages use each font, for the survey and the tests.
    #[cfg(test)]
    pub(crate) fn font_pages(&self) -> &BTreeMap<ObjectId, std::collections::BTreeSet<usize>> {
        &self.fonts
    }
}

/// Count every place each Form XObject is drawn.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `resource-graph-cycle` or `resource-graph-depth` if the graph
/// is cyclic or nests past [`MAX_RESOURCE_DEPTH`]; [`Error::Malformed`] for a page tree that
/// does not terminate. Whatever the engine failed with.
pub(crate) fn count_form_uses(
    document: &Document,
    deadline: &Deadline,
    clock: &Arc<dyn Clock>,
) -> Result<FormUseCounts> {
    let mut walk = Walk {
        document,
        from_pages: BTreeMap::new(),
        edges: BTreeMap::new(),
        descended: BTreeSet::new(),
        open: Vec::new(),
        container: None,
        fonts_read: BTreeSet::new(),
        dictionaries_read: 0,
        page_fonts: BTreeMap::new(),
        container_fonts: BTreeMap::new(),
        page_reaches: BTreeMap::new(),
        at_page: 0,
    };
    let pages = document.page_count()?;
    for index in 0..pages {
        // THE PAGE BOUNDARY `Limits` PROMISES, as `rotate` does for its ancestor climb. One
        // check per page: the walk below is a `/Parent` climb plus a resource graph plus an
        // `unparse` per dictionary, and no engine here offers a timeout or an abort hook
        // (ADR 0007), so a page is the finest grain available. `max_duration_ms` is
        // cooperative, which means a path that never checks it cannot honour it.
        deadline.checkpoint(clock.as_ref())?;
        let at = usize::try_from(index)
            .map_err(|_| Error::Internal("qpdf: a page index that is not an index".to_owned()))?;
        // SAFETY: `at` is below the page count just read from this document.
        let page = unsafe { ObjectHandle::page(document, at) };
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        walk.at_page = at;
        walk.page(&page)?;
    }
    Ok(FormUseCounts {
        counts: propagate(&walk.from_pages, &walk.edges),
        fonts: join_fonts_to_pages(&walk),
    })
}

/// Which pages reach each font, joining the per-container sets over the reference graph.
///
/// A font named inside a form that page 3 draws is a font page 3 uses, and cutting it changes
/// page 3 — so the closure matters and a direct-references-only join would under-report it,
/// which is the direction that cuts a font somebody else still needs.
fn join_fonts_to_pages(walk: &Walk<'_>) -> BTreeMap<ObjectId, std::collections::BTreeSet<usize>> {
    let mut fonts: BTreeMap<ObjectId, std::collections::BTreeSet<usize>> = BTreeMap::new();
    for (page, direct) in &walk.page_reaches {
        // Everything this page reaches, transitively. Bounded by the graph, which the cycle
        // check has already established is acyclic.
        let mut seen: std::collections::BTreeSet<ObjectId> = direct.clone();
        let mut stack: Vec<ObjectId> = direct.iter().copied().collect();
        while let Some(object) = stack.pop() {
            for child in walk
                .edges
                .get(&object)
                .into_iter()
                .flatten()
                .map(|(c, _)| *c)
            {
                if seen.insert(child) {
                    stack.push(child);
                }
            }
        }
        for container in &seen {
            for font in walk.container_fonts.get(container).into_iter().flatten() {
                fonts.entry(*font).or_default().insert(*page);
            }
        }
    }
    for (page, named) in &walk.page_fonts {
        for font in named {
            fonts.entry(*font).or_default().insert(*page);
        }
    }
    fonts
}

/// Push the page-level counts down the reference graph.
///
/// `uses(child) = direct references from pages + sum over parents of uses(parent) x references`.
///
/// Relaxed [`MAX_RESOURCE_DEPTH`] + 1 times rather than topologically sorted: the graph is a DAG
/// (cycles are refused during collection) whose longest path the depth cap already bounds, so
/// that many rounds is enough and the argument is one sentence rather than a sort nobody checks.
fn propagate(
    from_pages: &BTreeMap<ObjectId, usize>,
    edges: &BTreeMap<ObjectId, BTreeMap<ObjectId, usize>>,
) -> BTreeMap<ObjectId, usize> {
    let mut counts = from_pages.clone();
    for _ in 0..=MAX_RESOURCE_DEPTH {
        let mut next = from_pages.clone();
        for (parent, children) in edges {
            let parent_uses = counts.get(parent).copied().unwrap_or(0);
            if parent_uses == 0 {
                continue;
            }
            for (child, references) in children {
                *next.entry(*child).or_insert(0) += parent_uses.saturating_mul(*references);
            }
        }
        if next == counts {
            break;
        }
        counts = next;
    }
    counts
}

/// The walk's state: what it has counted, what it has already descended, and what is open.
struct Walk<'a> {
    document: &'a Document,
    /// Forms referenced straight from a page's resources or a page's annotations.
    from_pages: BTreeMap<ObjectId, usize>,
    /// `container -> form -> how many times that container references it`.
    ///
    /// Collected rather than counted, so multiplicity can be propagated afterwards. Counting
    /// during the walk and descending once undercounts every nested form; see
    /// [`FormUseCounts`].
    edges: BTreeMap<ObjectId, BTreeMap<ObjectId, usize>>,
    /// Subtrees already descended, so a form referenced *n* times costs its subtree once. Safe
    /// now that multiplicity comes from the propagation rather than from the traversal.
    descended: BTreeSet<ObjectId>,
    /// The objects on the current path, for cycle detection. **Not** the same set as
    /// `descended`: a form legitimately appearing twice is sharing, not a cycle, and a walk
    /// that confused the two would refuse the documents this exists to measure.
    open: Vec<ObjectId>,
    /// Which stream's resources are being read, or `None` at page level.
    container: Option<ObjectId>,
    /// Font objects whose `/Resources` have already been read, so a branching ladder of Type 3
    /// fonts costs each rung once rather than once per path to it.
    fonts_read: BTreeSet<ObjectId>,
    /// How many resource dictionaries have been read, against [`MAX_RESOURCE_DICTIONARIES`].
    dictionaries_read: usize,
    /// Fonts each page names directly, and fonts each container names.
    ///
    /// # Collected separately, then joined
    ///
    /// The memo that makes this walk linear -- a subtree is descended once -- is right for
    /// counting form *references*, which are propagated afterwards. It is wrong for a font's
    /// **page set**: a subtree skipped on page 7 because page 3 already walked it would lose
    /// page 7 from every font inside it, and the rule would then cut a font page 7 still uses.
    /// Clearing the memo per page fixes that and double-counts every form edge instead;
    /// measured, it broke the nested-form fixture.
    ///
    /// So the page set is computed from the graph rather than during it: which fonts each
    /// container names, which objects each page reaches, and a closure over the edges.
    page_fonts: BTreeMap<usize, std::collections::BTreeSet<ObjectId>>,
    container_fonts: BTreeMap<ObjectId, std::collections::BTreeSet<ObjectId>>,
    page_reaches: BTreeMap<usize, std::collections::BTreeSet<ObjectId>>,
    /// The page whose subtree is being walked.
    at_page: usize,
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
        self.dictionaries_read += 1;
        if self.dictionaries_read > MAX_RESOURCE_DICTIONARIES {
            return Err(Error::Unsupported(
                "qpdf sharing [resource-graph-work]: more resource dictionaries than burrow \
                 will read, which a branching graph reaches long before the depth cap"
                    .to_owned(),
            ));
        }
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
            match self.container {
                None => {
                    *self.from_pages.entry(object).or_insert(0) += 1;
                    self.page_reaches
                        .entry(self.at_page)
                        .or_default()
                        .insert(object);
                }
                Some(parent) => {
                    *self
                        .edges
                        .entry(parent)
                        .or_default()
                        .entry(object)
                        .or_insert(0) += 1;
                }
            }
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
        let enclosing = self.container.replace(object);
        let result = self.resources(&resources, depth + 1);
        self.container = enclosing;
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
            // EVERY FONT, not only the Type 3 ones: the page set is what decides whether a
            // font may be cut, and a TrueType font shared with another page matters exactly as
            // much as a Type 3 one. Recorded against the CONTAINER, and joined to pages after
            // the walk -- see `Walk::page_fonts`.
            let identity = font.object()?;
            match self.container {
                None => {
                    self.page_fonts
                        .entry(self.at_page)
                        .or_default()
                        .insert(identity);
                }
                Some(parent) => {
                    self.container_fonts
                        .entry(parent)
                        .or_default()
                        .insert(identity);
                }
            }

            let Some(procs) = self.dictionary_key(&font, &CHARPROCS)? else {
                continue;
            };
            // ONCE PER FONT OBJECT. Without this a branching ladder of Type 3 fonts is
            // exponential in depth: `MAX_RESOURCE_DEPTH` caps the path and nothing capped the
            // number of paths. See `MAX_RESOURCE_DICTIONARIES`.
            let identity = font.object()?;
            if self.fonts_read.insert(identity) {
                // The font's own `/Resources` is what its procedures draw against.
                if let Some(inner) = self.dictionary_key(&font, &RESOURCES)? {
                    self.resources(&inner, depth + 1)?;
                }
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
        // THE CONTAINER'S TYPE FIRST. `qpdf_oh_get_key` on a non-dictionary reaches
        // `QPDFObjectHandle::typeWarning` -> `Common::warn`, which appends to qpdf's warning
        // vector **whatever `suppress_warnings` says** -- that flag only stops the printing.
        // burrow sets no `max_warnings`, so every one is retained.
        //
        // Measured by a review: an `/Annots` array of 400,000 integers, 800 kB of file, peaked
        // at **265 MB** and took 401 ms; the same array of empty dictionaries grew nothing.
        // Roughly 330x amplification, linear in the array, inside the engine thread.
        let container = object.type_code();
        if let Some(error) = self.document.take_error() {
            return Err(error);
        }
        if container != object_type::DICTIONARY && container != object_type::STREAM {
            return Ok(None);
        }
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

/// The counts, as the geometry layer's sharing rule asks for them.
///
/// `pdfsyntax` holds the rule and knows nothing about qpdf object identities, so the packing
/// happens here — the same `(number << 16) | generation` shape `Form::id` uses, which is what
/// the walk put on every glyph.
impl crate::pdfsyntax::geometry::FormUses for FormUseCounts {
    fn uses(&self, form: u64) -> Result<usize> {
        let number = i32::try_from(form >> 16).map_err(|_| {
            Error::Internal("pdf sharing: a form identity that is not an object number".to_owned())
        })?;
        let generation = i32::try_from(form & 0xffff).map_err(|_| {
            Error::Internal("pdf sharing: a form identity with no generation".to_owned())
        })?;
        Ok(self.uses((number, generation)))
    }
}
