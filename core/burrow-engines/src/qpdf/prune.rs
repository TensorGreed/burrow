//! Removing what the copy dragged along.
//!
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) §2b, issue #54.
//! `qpdf_add_page` copies a **reachability closure**, not a page: `Pages::insert` →
//! `Copier::reserve_objects` is an unbounded deep traversal whose only stopping rules are
//! `/Pages` nodes and other page objects. Everything else an included page can reach comes too,
//! whoever else owned it — an inherited resource dictionary, a form field whose siblings live on
//! excluded pages, an annotation array shared by two pages, an article bead.
//!
//! So building is not enough, and this is the second half: **build, then ask separately what the
//! copy dragged along.** The two questions are not the same one.
//!
//! # The rule, and the one place it is an allowlist
//!
//! §2b: for each thing an included page reaches that also belongs to excluded content, prune it
//! correctly or drop it entirely — never carry it through wholesale.
//!
//! | what | what happens here | which answer |
//! |---|---|---|
//! | page keys | **everything not on [`KEPT_PAGE_KEYS`] is removed** | allowlist |
//! | `/Annots` shared with an excluded page | annotations that cannot show a `/P` naming a page **in this output** are erased | prune |
//! | a widget's `/Parent` | removed, always | drop |
//! | an annotation's `/A`, `/Dest`, `/AA` | removed, always | drop |
//! | inherited `/Resources` | filtered to the names the copied streams actually mention | prune |
//! | optional content | **the split is refused** | neither — see below |
//!
//! **The page-key row is the load-bearing one**, and it is why this module reads a dictionary's
//! keys rather than removing a list of them. `/B` (article beads), `/AA`, `/Thumb`, `/PieceInfo`,
//! `/StructParents` and page-level `/Metadata` all go through it, and so does every key nobody
//! enumerated. A denylist over a structure whose design permits keys nobody enumerated is exactly
//! what ADR 0019's *Alternatives considered* rejects carve for; taking it here would be the same
//! mistake one level down.
//!
//! # Optional content is refused, and that is a product decision rather than a shortcut
//!
//! §2b forbids dropping `/OCProperties` alone: the configuration that turns a layer *off* lives on
//! the catalog and the content does not, so an output with the layers and without the configuration
//! shows content the source hid. Carrying a pruned configuration is the other answer, and **burrow
//! cannot reach a destination's catalog at all**: `qpdf_get_root` is `QTC::TC(…); return
//! trap_oh_errors(…)` — two top-level statements, so ADR 0013's caller rule refuses it, and it is
//! not on `engines/qpdf-trapped-functions.txt`.
//!
//! Neither §2b answer is available, so this takes a third that is stronger than both: a document
//! whose kept pages reference optional content is **not split**. "Reference" is checked at every
//! depth the resource walk reaches — the page's own `/Resources /Properties` and `/XObject`, each
//! annotation's `/OC`, and then every nested Form XObject, tiling pattern, Type 3 glyph procedure
//! and appearance stream it follows. The one-level version shipped in review and the module header
//! claimed the channel closed; it did not say "one level", which is how a reader would have been
//! told a gap was covered. ADR 0019 §4 has the sentence
//! `/split-pdf` will carry when that page exists; there is no such page yet, and saying "the page
//! says so" in the present tense would be describing one that does not. A refusal is a correct
//! answer; a file that quietly reveals what somebody hid is not.
//!
//! # What transfers to redaction, and what is `split`-only
//!
//! Everything in this module is about *an output that contains some pages and not others*, which is
//! redaction's shape too — it removes content from a page rather than pages from a document, and
//! then has the identical question about what the remaining objects still reach.
//! [`prune_page`] takes a page and a fact about it, not a page range, for that reason.
//! `split`-only is the caller: which pages are in this output, and how the runs were computed.
//!
//! # What this does NOT do, stated rather than discovered
//!
//! - **Nested resource dictionaries are not pruned.** A Form XObject's own `/Resources` survives
//!   whole. It is reachable only through an XObject the page uses, so it belongs to this page —
//!   unless the XObject itself is shared with an excluded page, which is the sub-object blind spot
//!   ADR 0019 §3 records and the named canaries rather than the structural harness cover.
//! - **The name filter over-approximates.** See `crate::pdfsyntax::names`: a name is collected
//!   because it appears, so a resource named in drawn text survives. Larger output, not a leak.
//! - **Nothing here sees inside `/DCTDecode`, `/JPXDecode` or `/JBIG2Decode`.** A leak present only
//!   in transformed form is outside this module, outside `verify`, and outside `qpdf --qdf`. For
//!   redaction that transformed form *is* the leak, so M2 must not inherit this as sufficient.

use core::ffi::c_int;
use std::collections::{BTreeMap, BTreeSet};

use burrow_types::{Deadline, Error, Result};

use super::Document;
use super::handle::ObjectHandle;
use crate::codes::qpdf::object_type;
use crate::pdfsyntax::{names_in_content, top_level_keys};

/// The keys a page in a split output may keep.
///
/// **An allowlist, and the only one in this module.** Everything else on the page dictionary is
/// removed, whether or not anybody has heard of it.
///
/// Each entry is here because a page needs it to be the page it was:
///
/// | | |
/// |---|---|
/// | `/Type`, `/Parent` | structure. Removing `/Parent` detaches the page from its own tree |
/// | `/Contents`, `/Resources` | what the page draws, and what it draws with |
/// | `/MediaBox`, `/CropBox`, `/BleedBox`, `/TrimBox`, `/ArtBox` | its size and its boxes |
/// | `/Rotate` | how it displays — and the witness ADR 0022's verification compares |
/// | `/Group` | transparency group; without it a page with soft masks composites wrongly |
/// | `/Annots` | its annotations, themselves filtered below |
/// | `/UserUnit`, `/Tabs` | a scale factor and a tab order, both self-contained |
///
/// Notable absences, each of which is a channel:
///
/// - **`/B`** — article beads, §2a row 4. A bead reaches its thread, and the thread's `/I` info
///   dictionary names the article. Dropped with `/Threads`, which lives on the catalog the build
///   route never copies.
/// - **`/StructParents`** — an index into a structure tree the output does not have. Harmless
///   alone and meaningless without its tree.
/// - **`/Metadata`** — page-level XMP, which producers routinely fill with document-wide text.
/// - **`/AA`, `/Thumb`, `/PieceInfo`, `/PresSteps`, `/VP`, `/LastModified`** — and anything else.
///   They are not listed as exclusions because nothing enumerates them: they are simply not here.
const KEPT_PAGE_KEYS: &[&[u8]] = &[
    b"Type",
    b"Parent",
    b"Contents",
    b"Resources",
    b"MediaBox",
    b"CropBox",
    b"BleedBox",
    b"TrimBox",
    b"ArtBox",
    b"Rotate",
    b"Group",
    b"Annots",
    b"UserUnit",
    b"Tabs",
];

/// Keys removed from every annotation that survives.
///
/// A **rule list rather than an allowlist**, and the difference is stated rather than glossed: an
/// annotation's legitimate key set is large, producer-specific and per-subtype, so an allowlist
/// here would delete things real documents need and would have to grow every time somebody noticed.
/// The page dictionary is the opposite case — a small, specified key set — which is why that one is
/// an allowlist and this one is not.
///
/// What each closes:
///
/// - **`/Parent`** — §2a row 2, the worst of the six. A widget's parent is the form field, and the
///   field carries `/T` (its name), `/V` (**the value somebody typed**) and `/Kids` reaching every
///   sibling widget, including the ones on pages this output does not contain. Cutting the edge
///   leaves the field unreferenced, so qpdf's writer never emits it.
/// - **`/A` and `/Dest`** — §2a row 5. A named destination *is* a name, and names are descriptive;
///   an explicit destination points at a page object the copier stopped at, so it resolves to
///   nothing anyway. Both are dropped, which is §2b's "drop the action, leaving a link that does
///   nothing".
/// - **`/AA`** — additional actions, the same leak through a different key.
/// - **`/StructParent`** — as `/StructParents` on the page.
const REMOVED_ANNOTATION_KEYS: &[&[u8]] = &[b"Parent", b"A", b"Dest", b"AA", b"StructParent"];

const RESOURCES_KEY: &[u8] = b"/Resources\0";
const ANNOTS_KEY: &[u8] = b"/Annots\0";
const P_KEY: &[u8] = b"/P\0";
const OC_KEY: &[u8] = b"/OC\0";
const TYPE_KEY: &[u8] = b"/Type\0";
const AP_KEY: &[u8] = b"/AP\0";
const CHARPROCS_KEY: &[u8] = b"/CharProcs\0";

/// For each source page, the **other** source pages its `/Annots` array is shared with.
///
/// **Computed on the source, once, before anything is copied**, because it cannot be recovered
/// afterwards: the destination holds a copy of the array, and a copy shared with an *excluded*
/// page looks exactly like a copy shared with nobody.
///
/// # Why the sharers rather than a yes/no
///
/// The first version of this returned `Vec<bool>` — "is this array shared at all" — and a
/// one-way split then dropped every annotation in the document. Sharing is not the thing that
/// makes an annotation ambiguous; sharing with a page **this output does not contain** is. When
/// every page that shares the array is in the output, each annotation is on one of them, and
/// nothing about it belongs to content that was excluded. Measured: the closure harness's
/// `a_one_way_split_loses_no_page_content` failed on an operation that excluded nothing.
///
/// # Why sharedness decides the rule rather than `/P` alone
///
/// The obvious filter is "keep annotations whose `/P` names this page". It is wrong on real
/// documents: `/P` is optional (PDF 32000-1 §12.5.2) and producers routinely omit it, so that
/// rule would delete most annotations from most files — including the kept page's own, which is
/// the degenerate way to pass a leak test.
///
/// So `/P` is required only where it is needed. An array that reaches no excluded page is kept
/// whole; one that does is ambiguous, and there an annotation that cannot prove it belongs here
/// does not travel.
///
/// # Errors
///
/// Whatever qpdf latched while reading a page or its `/Annots`.
pub(super) fn annots_sharing(
    source: &Document,
    pages: u64,
    options: &crate::OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<Vec<Vec<u64>>> {
    // THE OPEN'S DEADLINE, PER PAGE. This sweep is O(source pages) engine calls, so on a
    // `max_pages`-sized document an uncheckpointed version sits outside every ceiling -- the
    // same defect ADR 0022 measured in `rotations` at 144 ms of unchecked sweep. The budget is
    // the one `open_document` started, not a new one.
    let clock = std::sync::Arc::clone(&options.clock);
    let count = usize::try_from(pages)
        .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
    let mut identities: Vec<Option<(c_int, c_int)>> = Vec::with_capacity(count);
    for index in 0..count {
        deadline.checkpoint(clock.as_ref())?;
        // SAFETY: `index` is below the document's page count, which the caller established.
        let page = unsafe { ObjectHandle::page(source, index) };
        if let Some(error) = source.take_error() {
            return Err(error);
        }
        let annots = page.key(source, ANNOTS_KEY.as_ptr().cast());
        if let Some(error) = source.take_error() {
            return Err(error);
        }
        if annots.type_code() == object_type::ARRAY {
            identities.push(Some(annots.object(source)?));
        } else {
            identities.push(None);
        }
    }

    // A DIRECT `/Annots` ARRAY IS NEVER SHARED, and it reads as shared here if identity is taken
    // carelessly: `object()` returns (0, 0) for a direct object, so every page with an inline
    // array would match every other one and each would have every other as a "sharer".
    let mut sharers = Vec::with_capacity(identities.len());
    for (at, identity) in identities.iter().enumerate() {
        let mut with: Vec<u64> = Vec::new();
        if let Some(id) = identity
            && *id != (0, 0)
        {
            for (other, candidate) in identities.iter().enumerate() {
                if other != at && candidate.as_ref() == Some(id) {
                    with.push(u64::try_from(other).map_err(|_| {
                        Error::Internal("page index does not fit in u64".to_owned())
                    })?);
                }
            }
        }
        sharers.push(with);
    }
    Ok(sharers)
}

/// Remove everything from this output that belongs to content it does not contain.
///
/// `ambiguous[i]` says whether page `i` of the output has an `/Annots` array shared with a source
/// page the output does **not** contain — derived by the caller from [`annots_sharing`], which is
/// read on the source before the copy.
///
/// # Why this takes the whole output rather than one page
///
/// Because the objects it prunes are **shared between pages**, which is the entire subject of
/// ADR 0019 §2a, and a per-page pass gets that wrong in the most damaging possible way.
///
/// `qpdf_add_page` flattens the page tree and pushes inherited attributes down (ADR 0021), and it
/// pushes the *reference*: every page under a `/Pages` node that carried `/Resources` ends up with
/// `/Resources 11 0 R` — **the same object**. A pass that pruned it once per page would compute
/// page 1's used names, delete everything else from the dictionary, and then hand page 4 a
/// dictionary with its font already gone.
///
/// That is not hypothetical. It is what the first version of this module did, and
/// `a_one_way_split_loses_no_page_content` caught it: a split that excluded **nothing** deleted the
/// one resource the fixture's inherited dictionary held. The same argument applies to a shared
/// `/Annots` array, where the per-page version would erase page 2's annotations while filtering
/// page 1's.
///
/// So resources are pruned **once per dictionary**, against the union of the names used by every
/// page in this output that shares it; and an annotation on a shared array is kept if it belongs to
/// **any** page in this output.
///
/// # Errors
///
/// - [`Error::Unsupported`] — a page references optional content, or a bound in this module or in
///   `crate::pdfsyntax` was reached. Every one of those is a refusal rather than a partial prune.
/// - [`Error::Malformed`] — a dictionary qpdf produced could not be read back.
/// - Whatever qpdf latched.
pub(super) fn prune_output(
    dest: &Document,
    ambiguous: &[bool],
    options: &crate::OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<()> {
    // THE OPERATION'S DEADLINE, CHECKPOINTED PER PAGE AND PER STREAM. This pass decodes every
    // content stream every page reaches, and it had no time ceiling at all: security review
    // measured a 155 kB document of 1,000 pages sharing one form at 176 seconds, with resident
    // memory never above 65 MB -- so `max_memory_bytes` never fired either, and `split` consults
    // its deadline only between outputs, which for a one-part split is never.
    let clock = std::sync::Arc::clone(&options.clock);
    let mut walk = Walk {
        seen: BTreeMap::new(),
        budget: MAX_STREAMS_PER_OUTPUT,
        deadline,
        clock: clock.as_ref(),
    };
    let mut pages = Vec::with_capacity(ambiguous.len());
    for at in 0..ambiguous.len() {
        // SAFETY: `at` is below the destination's page count -- the caller added exactly
        // `ambiguous.len()` pages and removed the blank one. Routes through `trap_errors`.
        let page = unsafe { ObjectHandle::page(dest, at) };
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        pages.push(page);
    }

    // THE PAGES THIS OUTPUT CONTAINS, by object identity rather than by handle: a handle is a
    // fresh number on every call and two handles to the same page never compare equal (ADR
    // 0013). This set is what an annotation on a shared array has to be in.
    let mut in_output: BTreeSet<(c_int, c_int)> = BTreeSet::new();
    for page in &pages {
        in_output.insert(page.object(dest)?);
    }

    // Pass one: annotations, the optional-content refusal, and the used-name set per page.
    // Resource dictionaries are collected rather than pruned, because a shared one's answer is
    // not known until every page that shares it has been read.
    let mut resource_groups: Vec<(ObjectHandle<'_>, BTreeSet<Vec<u8>>)> = Vec::new();
    let mut group_of: BTreeMap<(c_int, c_int), usize> = BTreeMap::new();
    for (at, page) in pages.iter().enumerate() {
        walk.deadline.checkpoint(walk.clock)?;
        // NOT `unwrap_or(false)`. It is unreachable -- the loop is over `ambiguous.len()` --
        // and `false` is the LEAKING direction, which is the one default this module may not
        // take. An unreachable branch that fails safe costs nothing; one that fails open is a
        // leak waiting for the loop bound to change.
        let shared = *ambiguous.get(at).ok_or_else(|| {
            Error::Internal("a destination page has no recorded ambiguity".to_owned())
        })?;
        prune_annotations(dest, page, shared, &in_output)?;
        refuse_optional_content(dest, page)?;

        let resources = page.key(dest, RESOURCES_KEY.as_ptr().cast());
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        if resources.type_code() != object_type::DICTIONARY {
            // Nothing to filter. A page with no resources inherits none either: the flattening
            // above is what turns §2a's inherited-`/Resources` channel into a key on this
            // dictionary in the first place.
            continue;
        }
        let used = used_names(dest, page, &resources, &mut walk)?;
        let identity = resources.object(dest)?;
        if identity == (0, 0) {
            // A DIRECT dictionary belongs to this page alone and cannot be shared, so it is
            // pruned here rather than grouped -- grouping by (0, 0) would merge every direct
            // dictionary in the output into one group and keep each page's names on all of
            // them, which is a leak rather than a loss.
            prune_resource_dictionary(dest, &resources, &used)?;
            continue;
        }
        match group_of.get(&identity) {
            Some(index) => {
                if let Some((_, names)) = resource_groups.get_mut(*index) {
                    names.extend(used);
                }
            }
            None => {
                group_of.insert(identity, resource_groups.len());
                resource_groups.push((resources, used));
            }
        }
    }

    // Pass two: each shared resource dictionary, once, against the union.
    for (resources, used) in &resource_groups {
        prune_resource_dictionary(dest, resources, used)?;
    }

    // Pass three: the pages' own keys. LAST, because the passes above read `/Annots` and
    // `/Resources`, and a future key that is read and then removed would break silently if this
    // ran first.
    for page in &pages {
        prune_page_keys(dest, page)?;
    }
    Ok(())
}

/// Remove every page key the allowlist does not name.
fn prune_page_keys(dest: &Document, page: &ObjectHandle<'_>) -> Result<()> {
    if page.type_code() != object_type::DICTIONARY {
        return Err(Error::Malformed(
            "qpdf: a page that is not a dictionary".to_owned(),
        ));
    }
    // `unparse`, never `unparse_resolved`: this leaves indirect references as `N G R`, so the
    // answer is the page's own keys rather than the whole document behind them.
    for key in top_level_keys(&page.unparse())? {
        if KEPT_PAGE_KEYS.contains(&key.as_slice()) {
            continue;
        }
        let name = c_key(&key)?;
        page.remove_key(name.as_ptr().cast());
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
    }
    Ok(())
}

/// Filter `/Annots`, and strip the leaking keys from every annotation that stays.
///
/// `ambiguous` is "this array is shared with a page the output does not contain". Only then does
/// an annotation have to prove, through `/P`, that it belongs to a page in this output.
///
/// **`in_output`, not this page**, and the difference is a bug the closure harness caught. A
/// shared array is filtered once per page that references it, so a rule keyed on *this* page would
/// erase page 2's annotations while filtering page 1's, and then erase page 1's while filtering
/// page 2's — leaving an output that contains both pages and neither's annotations.
fn prune_annotations(
    dest: &Document,
    page: &ObjectHandle<'_>,
    ambiguous: bool,
    in_output: &BTreeSet<(c_int, c_int)>,
) -> Result<()> {
    let annots = page.key(dest, ANNOTS_KEY.as_ptr().cast());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if annots.type_code() != object_type::ARRAY {
        return Ok(());
    }
    // BACKWARDS. `qpdf_oh_erase_item` shifts everything after the erased item down by one, so a
    // forward loop skips the item that takes the removed one's place -- and the item it skips is,
    // by construction, the next annotation the filter was about to examine.
    let mut at = annots.array_len();
    while at > 0 {
        at -= 1;
        let annot = annots.array_item(dest, at);
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        if ambiguous && !belongs_to(dest, &annot, in_output)? {
            annots.erase_item(at);
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            continue;
        }
        if annot.type_code() != object_type::DICTIONARY {
            continue;
        }
        for key in REMOVED_ANNOTATION_KEYS {
            let name = c_key(key)?;
            annot.remove_key(name.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Whether `annot` says it is on one of the pages `in_output` identifies.
///
/// **Object number and generation, never a handle** — a handle is a fresh number on every call and
/// two handles to the same object never compare equal (ADR 0013's handle-identity amendment). An
/// always-false comparison here erases every annotation; an always-true one erases none and leaks.
fn belongs_to(
    dest: &Document,
    annot: &ObjectHandle<'_>,
    in_output: &BTreeSet<(c_int, c_int)>,
) -> Result<bool> {
    if annot.type_code() != object_type::DICTIONARY {
        // Not an annotation at all. On a shared array it cannot prove it belongs here.
        return Ok(false);
    }
    let owner = annot.key(dest, P_KEY.as_ptr().cast());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if owner.type_code() == object_type::NULL {
        return Ok(false);
    }
    Ok(in_output.contains(&owner.object(dest)?))
}

/// Refuse a page that references optional content.
///
/// See the module header: neither of ADR 0019 §2b's answers is reachable without the destination's
/// catalog, and the failure mode of getting it wrong is content the source **hid** arriving visible.
fn refuse_optional_content(dest: &Document, page: &ObjectHandle<'_>) -> Result<()> {
    let refusal = optional_content_refusal;

    // An annotation may be optional content in its own right.
    let annots = page.key(dest, ANNOTS_KEY.as_ptr().cast());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if annots.type_code() == object_type::ARRAY {
        let mut at = annots.array_len();
        while at > 0 {
            at -= 1;
            let annot = annots.array_item(dest, at);
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            if has_oc(dest, &annot)? {
                return Err(refusal());
            }
        }
    }

    let resources = page.key(dest, RESOURCES_KEY.as_ptr().cast());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if resources.type_code() != object_type::DICTIONARY {
        return Ok(());
    }

    // `/Properties` is where `BDC /OC /Name` resolves. It also holds ordinary marked-content
    // property lists, so the TYPE decides rather than the category's presence -- refusing every
    // document with a `/Properties` entry would refuse tagged PDFs, which have nothing hidden.
    refuse_optional_content_in(dest, &resources)?;

    // And an XObject may carry `/OC` on its own stream dictionary.
    let xobjects = resources.key(dest, c"/XObject".as_ptr());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if xobjects.type_code() == object_type::DICTIONARY {
        for key in top_level_keys(&xobjects.unparse())? {
            let name = c_key(&key)?;
            let entry = xobjects.key(dest, name.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            if has_oc(dest, &entry)? {
                return Err(refusal());
            }
        }
    }
    Ok(())
}

/// Refuse if any `/Properties` entry in `resources` is an optional content group.
///
/// The half of the optional-content check that applies to a `/Resources` dictionary wherever it
/// is found — the page's own, or a nested Form XObject's. It is a function rather than two copies
/// because the nested case was missing entirely and the module header said the channel was closed:
/// security review measured a document whose hidden layer lived one level down splitting happily,
/// with the hidden text visible in the output.
///
/// # Errors
///
/// [`Error::Unsupported`] if an entry is an `/OCG` or `/OCMD`, and whatever qpdf latched.
fn refuse_optional_content_in(dest: &Document, resources: &ObjectHandle<'_>) -> Result<()> {
    let properties = resources.key(dest, c"/Properties".as_ptr());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if properties.type_code() != object_type::DICTIONARY {
        return Ok(());
    }
    for key in top_level_keys(&properties.unparse())? {
        let name = c_key(&key)?;
        let entry = properties.key(dest, name.as_ptr().cast());
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        if is_optional_content_group(dest, &entry)? {
            return Err(optional_content_refusal());
        }
    }
    Ok(())
}

/// The one refusal message, so the page-level pass and the nested walk cannot drift.
fn optional_content_refusal() -> Error {
    Error::Unsupported(
        "this document uses optional content (layers), and burrow cannot carry the setting that \
         decides whether a layer is hidden into a part of it"
            .to_owned(),
    )
}

/// Whether `object` carries a non-null `/OC`, looking through a stream to its dictionary.
fn has_oc(dest: &Document, object: &ObjectHandle<'_>) -> Result<bool> {
    let dictionary = match object.type_code() {
        object_type::DICTIONARY => object.key(dest, OC_KEY.as_ptr().cast()),
        object_type::STREAM => {
            let inner = object.stream_dict(dest);
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            inner.key(dest, OC_KEY.as_ptr().cast())
        }
        _ => return Ok(false),
    };
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    Ok(dictionary.type_code() != object_type::NULL)
}

/// Whether `entry` is an optional content group or membership dictionary.
fn is_optional_content_group(dest: &Document, entry: &ObjectHandle<'_>) -> Result<bool> {
    if entry.type_code() != object_type::DICTIONARY {
        return Ok(false);
    }
    let kind = entry.key(dest, TYPE_KEY.as_ptr().cast());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if kind.type_code() != object_type::NAME {
        return Ok(false);
    }
    let name = kind.name();
    Ok(name == b"/OCG" || name == b"/OCMD")
}
/// Filter one `/Resources` dictionary, keeping only what the output still uses.
///
/// **An allowlist over the dictionary's keys, not a filter inside seven categories.** The first
/// version iterated [`RESOURCE_CATEGORIES`] and filtered inside each, which meant any key that was
/// not one of the seven survived whole. Security review measured it: a `/Resources` holding
/// `/Font`, `/ProcSet` and `/Stash << /Secret … >>` came out with `/Font` correctly pruned and
/// `/Stash` — and its payload, belonging to an excluded page — untouched.
///
/// That is ADR 0019 §2a row 1 surviving through a key nobody enumerated, which is the precise
/// shape the page-key rule was made an allowlist to avoid. Doing it one level down and not the
/// other was an inconsistency rather than a design.
///
/// `/ProcSet` is kept whole because it is a list of names rather than a map to objects — it
/// reaches nothing and can carry nothing.
fn prune_resource_dictionary(
    dest: &Document,
    resources: &ObjectHandle<'_>,
    used: &BTreeSet<Vec<u8>>,
) -> Result<()> {
    for key in top_level_keys(&resources.unparse())? {
        let category = c_key(&key)?;
        let Some(kind) = ResourceCategory::of(&key) else {
            // NOT A CATEGORY THIS UNDERSTANDS. `/ProcSet` reaches nothing; anything else is a key
            // outside the specification's resource set, and the only safe thing to do with an
            // unknown map to objects is remove it.
            if key.as_slice() != b"ProcSet" {
                resources.remove_key(category.as_ptr().cast());
                if let Some(error) = dest.take_error() {
                    return Err(error);
                }
            }
            continue;
        };
        let _ = kind;
        let sub = resources.key(dest, category.as_ptr().cast());
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        if sub.type_code() != object_type::DICTIONARY {
            continue;
        }
        for name in top_level_keys(&sub.unparse())? {
            if used.contains(&name) {
                continue;
            }
            let entry = c_key(&name)?;
            sub.remove_key(entry.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// A resource category whose entries can be streams worth reading for further names.
///
/// **An image is not one of them, and getting that wrong refused every PDF containing a JPEG.**
/// The first version followed anything in `/XObject` that was a stream. `/XObject` holds images as
/// well as forms, so a `/DCTDecode` image reached `stream_data`, which does not decode lossy
/// filters at `qpdf_dl_specialized` and correctly reported "not decoded" — and the walk turned
/// that into a refusal. Measured by security review: **every** JPEG-bearing document. A
/// `/FlateDecode` image was worse, because it *did* decode, and its pixels then failed to lex as
/// PDF syntax roughly whenever they contained an unbalanced `(`.
///
/// An image contributes no resource names. Following one was never useful; it was only expensive
/// and then fatal.
#[derive(Clone, Copy)]
enum ResourceCategory {
    /// `/XObject` — followed only when `/Subtype` is `/Form`.
    XObject,
    /// `/Pattern` — followed only when `/PatternType` is 1, a tiling pattern with content.
    Pattern,
    /// `/Font` — a Type 3 font's `/CharProcs` are content streams.
    Font,
    /// Reached but never followed: these hold no content stream.
    Inert,
}

impl ResourceCategory {
    /// The category a resource dictionary key names, or `None` if it is not one.
    fn of(key: &[u8]) -> Option<Self> {
        match key {
            b"XObject" => Some(Self::XObject),
            b"Pattern" => Some(Self::Pattern),
            b"Font" => Some(Self::Font),
            b"Shading" | b"ColorSpace" | b"ExtGState" | b"Properties" => Some(Self::Inert),
            _ => None,
        }
    }
}

/// What the whole output's resource walk shares.
///
/// **Output-wide rather than per page, and that is a denial-of-service fix rather than a tidy-up.**
/// A form XObject shared by every page was decoded and lexed once per page, and the visited set
/// that would have stopped it was created per page. Security review measured a 155 kB document of
/// 1,000 pages sharing one 20 MB-inflating form at **176 seconds**, with resident memory never
/// above 65 MB — so `max_memory_bytes` never fired, and `max_duration_ms` was consulted only
/// between outputs, which for a one-part split is never.
///
/// Caching the name set per object removes the amplification rather than merely detecting it: a
/// stream's names do not depend on which page reached it.
struct Walk<'a> {
    /// Name sets already computed, by object identity.
    seen: BTreeMap<(c_int, c_int), BTreeSet<Vec<u8>>>,
    /// How many more streams this output may decode.
    budget: usize,
    /// The operation's deadline, checkpointed per stream.
    deadline: &'a Deadline,
    /// The clock that deadline is measured against.
    clock: &'a dyn burrow_types::Clock,
}

/// The most streams one output's walk will decode.
///
/// Per **output** now, not per page — with the cache above, a document whose pages share their
/// forms costs one decode each however many pages reach them.
const MAX_STREAMS_PER_OUTPUT: usize = 16_384;

/// The deepest this follows one form XObject into another.
///
/// A form may draw a form may draw a form. Bounding it means a document cannot spend this crate's
/// stack on a graph that nests into itself; the visited cache already stops an outright cycle.
const MAX_NESTED_STREAM_DEPTH: usize = 16;

/// The most names one page may accumulate across every stream it reaches.
///
/// `names_in_content` caps each stream's own set; nothing capped the union until security review
/// pointed out that it is the union which is held. A refusal rather than a truncation, for the
/// reason every other ceiling in this module is.
const MAX_NAMES_PER_PAGE: usize = 65_536;

/// Every name the page's own content, its annotations' appearances, and the streams those reach
/// mention.
///
/// **A fixpoint, because the graph is one**, and transitive over the *resource graph* rather than
/// over the page's dictionary. A nested form's own `/Resources` is walked with the names that
/// nested form mentions — which the first version did not do, so a font used only by a form two
/// levels down was pruned off the page while the form still asked for it. Security review measured
/// the file coming out with the font object gone entirely.
///
/// Every stream's names are unioned into one page-wide set, whether or not the stream had its own
/// `/Resources`. That over-approximates — a name resolving in a nested dictionary also keeps a
/// same-named entry on the page — and it is the direction that costs a larger file rather than a
/// broken page.
fn used_names(
    dest: &Document,
    page: &ObjectHandle<'_>,
    resources: &ObjectHandle<'_>,
    walk: &mut Walk<'_>,
) -> Result<BTreeSet<Vec<u8>>> {
    let mut used = names_in_content(&page.page_content(dest)?)?;

    // Appearance streams of the annotations that SURVIVED the filter. `prune_annotations` has
    // already run, so a dropped annotation's appearance contributes nothing -- which is the whole
    // reason for that ordering.
    let annots = page.key(dest, ANNOTS_KEY.as_ptr().cast());
    if let Some(error) = dest.take_error() {
        return Err(error);
    }
    if annots.type_code() == object_type::ARRAY {
        let count = annots.array_len();
        for at in 0..count {
            let annot = annots.array_item(dest, at);
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            let appearance = annot.key(dest, AP_KEY.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            absorb(dest, &appearance, Follow::Anything, &mut used, walk, 0)?;
        }
    }

    // The fixpoint over the page's own resources.
    loop {
        let before = used.len();
        let selector = used.clone();
        follow_resources(dest, resources, &selector, &mut used, walk, 0)?;
        if used.len() == before {
            return Ok(used);
        }
    }
}

/// Which streams a caller is willing to follow.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    /// Only a `/Subtype /Form` XObject.
    FormsOnly,
    /// Only a tiling pattern -- `/PatternType 1`.
    TilingOnly,
    /// Any stream. Glyph procedures and appearance streams, which carry no distinguishing key.
    Anything,
}

/// Walk one `/Resources` dictionary's stream-bearing categories for the names in `selector`.
fn follow_resources(
    dest: &Document,
    resources: &ObjectHandle<'_>,
    selector: &BTreeSet<Vec<u8>>,
    used: &mut BTreeSet<Vec<u8>>,
    walk: &mut Walk<'_>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_NESTED_STREAM_DEPTH {
        return Ok(());
    }
    for (category, follow) in [
        (&b"/XObject\0"[..], Follow::FormsOnly),
        (&b"/Pattern\0"[..], Follow::TilingOnly),
        (&b"/Font\0"[..], Follow::Anything),
    ] {
        let sub = resources.key(dest, category.as_ptr().cast());
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        if sub.type_code() != object_type::DICTIONARY {
            continue;
        }
        for key in top_level_keys(&sub.unparse())? {
            if !selector.contains(&key) {
                continue;
            }
            let name = c_key(&key)?;
            let entry = sub.key(dest, name.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            absorb(dest, &entry, follow, used, walk, depth + 1)?;
        }
    }
    Ok(())
}

/// Union the names of `object`, and of everything its own resources reach, into `used`.
fn absorb(
    dest: &Document,
    object: &ObjectHandle<'_>,
    follow: Follow,
    used: &mut BTreeSet<Vec<u8>>,
    walk: &mut Walk<'_>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_NESTED_STREAM_DEPTH {
        return Ok(());
    }
    match object.type_code() {
        object_type::STREAM => {
            let dictionary = object.stream_dict(dest);
            if let Some(error) = dest.take_error() {
                return Err(error);
            }

            // OPTIONAL CONTENT, AT EVERY DEPTH. The page-level refusal sees the page's own
            // `/Resources` and its annotations. A Form XObject's `/OC`, or an OCG in a NESTED
            // `/Resources /Properties`, is past it -- and security review measured a document
            // whose hidden layer lived one level down splitting happily, with the hidden text
            // visible in the output and the layer's name still in it. This walk already visits
            // exactly those streams, so the check rides along rather than becoming a second
            // traversal that could disagree with this one.
            if has_oc(dest, &dictionary)? {
                return Err(optional_content_refusal());
            }

            if !worth_following(dest, &dictionary, follow)? {
                // AN IMAGE, A SHADING PATTERN, A NON-TYPE-3 FONT FILE. None holds PDF syntax, so
                // none can name a resource. Following one is what refused every document
                // containing a JPEG.
                return Ok(());
            }

            let identity = object.object(dest)?;
            // A DIRECT STREAM CANNOT EXIST in PDF, so (0, 0) means qpdf could not identify this
            // one; it is followed rather than deduplicated, because treating it as already seen
            // would silently skip its names.
            if identity != (0, 0)
                && let Some(cached) = walk.seen.get(&identity)
            {
                let cached = cached.clone();
                absorb_into(used, cached)?;
                return Ok(());
            }

            walk.deadline.checkpoint(walk.clock)?;
            if walk.budget == 0 {
                return Err(Error::Unsupported(
                    "this output reaches more streams than burrow will follow to decide which \
                     resources its pages use"
                        .to_owned(),
                ));
            }
            walk.budget -= 1;

            let Some(data) = object.stream_data(dest)? else {
                // NOT "EMPTY". qpdf could not decode it, so the bytes are still compressed, and
                // lexing those for names yields accidents rather than the names that are there --
                // the under-approximation that deletes a resource the page draws with.
                return Err(Error::Unsupported(
                    "a stream on this page could not be decoded, so burrow cannot tell which \
                     resources the page uses"
                        .to_owned(),
                ));
            };
            let mine = names_in_content(&data)?;
            if identity != (0, 0) {
                walk.seen.insert(identity, mine.clone());
            }
            absorb_into(used, mine.clone())?;

            // ITS OWN `/Resources`, walked with ITS OWN names. This is what makes the walk
            // transitive over the resource graph rather than over the page's dictionary: a form
            // that draws another form reaches it only through here, and the second form's font
            // was pruned off the page while the second form still asked for it until this
            // existed.
            let own = dictionary.key(dest, RESOURCES_KEY.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            if own.type_code() == object_type::DICTIONARY {
                refuse_optional_content_in(dest, &own)?;
                follow_resources(dest, &own, &mine, used, walk, depth + 1)?;
            }

            // A Type 3 font's glyph procedures.
            let procs = dictionary.key(dest, CHARPROCS_KEY.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            absorb(dest, &procs, Follow::Anything, used, walk, depth + 1)
        }
        object_type::DICTIONARY => {
            // A Type 3 font dictionary, an `/AP` state dictionary, a `/CharProcs` map.
            let own = object.key(dest, RESOURCES_KEY.as_ptr().cast());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            if own.type_code() == object_type::DICTIONARY {
                refuse_optional_content_in(dest, &own)?;
            }
            for key in top_level_keys(&object.unparse())? {
                if key.as_slice() == b"Resources" {
                    continue;
                }
                let name = c_key(&key)?;
                let entry = object.key(dest, name.as_ptr().cast());
                if let Some(error) = dest.take_error() {
                    return Err(error);
                }
                absorb(dest, &entry, follow, used, walk, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Add `names` to `used`, refusing rather than truncating past the ceiling.
fn absorb_into(used: &mut BTreeSet<Vec<u8>>, names: BTreeSet<Vec<u8>>) -> Result<()> {
    used.extend(names);
    if used.len() > MAX_NAMES_PER_PAGE {
        return Err(Error::Unsupported(
            "a page reaches more distinct resource names than burrow will record".to_owned(),
        ));
    }
    Ok(())
}

/// Whether this stream holds PDF content worth lexing for names.
fn worth_following(dest: &Document, dictionary: &ObjectHandle<'_>, follow: Follow) -> Result<bool> {
    match follow {
        Follow::Anything => Ok(true),
        Follow::FormsOnly => {
            let subtype = dictionary.key(dest, c"/Subtype".as_ptr());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            if subtype.type_code() != object_type::NAME {
                return Ok(false);
            }
            Ok(subtype.name() == b"/Form")
        }
        Follow::TilingOnly => {
            let kind = dictionary.key(dest, c"/PatternType".as_ptr());
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
            if kind.type_code() != object_type::INTEGER {
                return Ok(false);
            }
            Ok(kind.integer_value() == 1)
        }
    }
}

/// A dictionary key as qpdf's C API wants it: canonicalised, leading `/`, NUL-terminated.
///
/// # Errors
///
/// [`Error::Malformed`] if the name contains a NUL. A C string would end there, so the call would
/// act on a *different, shorter* key — removing one the page needs, or failing to remove the one
/// that leaks. A name can contain a NUL legitimately, through `#00`; it cannot be passed through
/// this API, and refusing is the only answer that is not a guess.
fn c_key(name: &[u8]) -> Result<Vec<u8>> {
    if name.contains(&0) {
        return Err(Error::Malformed(
            "qpdf: a dictionary key containing a NUL cannot be addressed through the C API"
                .to_owned(),
        ));
    }
    let mut key = Vec::with_capacity(name.len() + 2);
    if !name.starts_with(b"/") {
        key.push(b'/');
    }
    key.extend_from_slice(name);
    key.push(0);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::{KEPT_PAGE_KEYS, REMOVED_ANNOTATION_KEYS, c_key};

    #[test]
    fn a_key_gets_its_slash_and_its_nul_exactly_once() {
        assert_eq!(c_key(b"Type").expect("plain"), b"/Type\0");
        // `qpdf_oh_get_name` hands back the leading slash and `top_level_keys` does not, and
        // both reach this function. `//Type` is a different key from `/Type`.
        assert_eq!(c_key(b"/Type").expect("already slashed"), b"/Type\0");
    }

    #[test]
    fn a_key_containing_a_nul_is_refused_rather_than_truncated() {
        // `/A#00B` is a legal PDF name. As a C string it is `/A`, so removing it would remove a
        // DIFFERENT key -- and leave the one that matters in place.
        assert!(c_key(b"A\0B").is_err());
    }

    #[test]
    fn the_page_allowlist_omits_every_channel_the_adr_names() {
        // Not an exclusion list in the code -- there is none, which is the point of an
        // allowlist -- so this is where the absences are asserted. `/B` is ADR 0019 §2a row 4.
        for leaking in [
            &b"B"[..],
            b"AA",
            b"Thumb",
            b"PieceInfo",
            b"StructParents",
            b"Metadata",
            b"PresSteps",
        ] {
            assert!(
                !KEPT_PAGE_KEYS.contains(&leaking),
                "{} is on the page allowlist and should not be",
                String::from_utf8_lossy(leaking)
            );
        }
    }

    #[test]
    fn the_page_allowlist_keeps_what_a_page_needs_to_be_itself() {
        // The other direction, and the one that fails LOUDLY rather than quietly: dropping
        // `/Contents` or `/MediaBox` produces a blank or wrongly-sized page.
        for needed in [
            &b"Type"[..],
            b"Parent",
            b"Contents",
            b"Resources",
            b"MediaBox",
            b"Rotate",
            b"Annots",
        ] {
            assert!(
                KEPT_PAGE_KEYS.contains(&needed),
                "{} is not on the page allowlist",
                String::from_utf8_lossy(needed)
            );
        }
    }

    #[test]
    fn the_annotation_rules_close_the_field_and_the_destination_channels() {
        // §2a rows 2 and 5. `/Parent` is the one that carries a typed value into a file that
        // does not contain the page it was typed on.
        for key in [&b"Parent"[..], b"A", b"Dest"] {
            assert!(
                REMOVED_ANNOTATION_KEYS.contains(&key),
                "{} is not removed from annotations",
                String::from_utf8_lossy(key)
            );
        }
    }
}
