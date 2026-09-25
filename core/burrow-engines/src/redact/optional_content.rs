//! Refuse a redaction of a page that uses optional content.
//!
//! [ADR 0029](../../../../docs/adr/0029-what-redaction-does-and-what-it-refuses-to-do.md) §3
//! assigns *optional content referenced by a kept page* to **refuse**, and §5 names its signal:
//! the page's resources reference an optional-content group, **followed transitively over the
//! resource graph**. `split` has had that refusal since M1, in `prune/mod.rs`. Redaction never
//! had it.
//!
//! # A gap the census could not show
//!
//! Until #166 every marked-content span named through `/Properties` refused as
//! `marked-content-properties-unresolved`, and an optional-content span is named that way by
//! construction (`/OC /MC0 BDC`). So `13-optional-content.pdf` refused — for a reason that was
//! not its reason — and the missing refusal behind it was invisible. Resolving the name removed
//! the accident, and an optional-content page whose layer the region did not cover would then
//! have redacted with nothing saying no. §3's decision was being honoured by a coincidence.
//!
//! # What it reads, and why so much
//!
//! The same reach as `prune`'s check, because it is the same rule:
//!
//! - every `/Properties` entry whose `/Type` is `/OCG` or `/OCMD`, in every resource dictionary
//!   the page reaches — its own, each form's, each tiling pattern's, each Type 3 font's;
//! - an `/OC` on any XObject's stream dictionary, **image or form** — a hidden image is hidden
//!   content too;
//! - an `/OC` on an annotation, and on its appearance streams, which are forms reached through
//!   `/Annots` rather than through `/Resources` and so are the reach a resources-only walk
//!   misses.
//!
//! What it does **not** read is a soft mask's group form, reached through `/ExtGState`: neither
//! `prune`'s check nor the sharing walk follows `/ExtGState` either, and a luminosity mask draws
//! no text a reader extracts. Stated so the reach above is read as a list, not as "everything".
//!
//! # Bounded by the graph, not by a ceiling
//!
//! Every stream, font and indirect resource dictionary is visited once, by identity, and the
//! walk is a work list rather than a recursion, so neither a cycle nor a deep chain can hold it.
//! The work is linear in the objects the page reaches, and the deadline is consulted at every
//! queued object. That per-object checkpoint is **not pinned by a test**: the sharing walk's page
//! checkpoints run first, and no test yet arranges a clock that trips inside this walk and
//! not before it. Stated rather than implied. It carries **no ceiling of its
//! own on purpose**: the sharing walk runs first, over every page, with a dictionary budget and a
//! depth cap, and a second ceiling here that the first always fires before would be a defence no
//! test can reach — the masking this crate measured once already, in `page_contents`.

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Result};

use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::redact::graph::PdfObject;

const ANNOTS: Name = Name::literal(b"/Annots\0");
const AP: Name = Name::literal(b"/AP\0");
const APPEARANCE_STATES: [Name; 3] = [
    Name::literal(b"/N\0"),
    Name::literal(b"/D\0"),
    Name::literal(b"/R\0"),
];
const OC: Name = Name::literal(b"/OC\0");
const TYPE: Name = Name::literal(b"/Type\0");
const RESOURCES: Name = Name::literal(b"/Resources\0");
const PROPERTIES: Name = Name::literal(b"/Properties\0");
const XOBJECT: Name = Name::literal(b"/XObject\0");
const PATTERN: Name = Name::literal(b"/Pattern\0");
const FONT: Name = Name::literal(b"/Font\0");
const OCG: Name = Name::literal(b"/OCG\0");
const OCMD: Name = Name::literal(b"/OCMD\0");
const SUBTYPE: Name = Name::literal(b"/Subtype\0");
const IMAGE: Name = Name::literal(b"/Image\0");

/// Refuse if `page`, drawing against `resources`, references optional content anywhere it reaches.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `optional-content`; `max_duration_ms` through the deadline; and
/// whatever reading a dictionary's keys failed with.
pub(crate) fn refuse_optional_content<O: PdfObject>(
    page: &O,
    resources: &O,
    deadline: &Deadline,
    clock: &Arc<dyn Clock>,
) -> Result<()> {
    let mut walk = Walk {
        seen: BTreeSet::new(),
        dictionaries: BTreeSet::new(),
        pending: Vec::new(),
        deadline,
        clock,
    };
    walk.annotations(page)?;
    walk.resources(resources)?;
    while let Some(item) = walk.pending.pop() {
        walk.deadline.checkpoint(walk.clock.as_ref())?;
        match item {
            Pending::Stream(stream) => walk.stream(&stream)?,
            Pending::Font(font) => walk.font(&font)?,
        }
    }
    Ok(())
}

/// Something reached and not yet read.
enum Pending<O> {
    /// A form, image, tiling pattern or appearance stream: its `/OC`, its own content's `/OC`
    /// marks unless it is an image, then its `/Resources`.
    Stream(O),
    /// A font: its `/Resources`, which a Type 3 font's glyph procedures draw against.
    Font(O),
}

struct Walk<'d, O> {
    /// Objects already queued, by identity. A graph that reaches one object by many routes
    /// reads it once, which is what makes the walk linear rather than exponential in a DAG.
    seen: BTreeSet<(core::ffi::c_int, core::ffi::c_int)>,
    /// Resource dictionaries already read, by identity -- **a memo of its own**.
    ///
    /// One set served both, and an object is legitimately both: a dictionary queued as the page's
    /// *font* was later skipped as an appearance stream's *`/Resources`*, so a typed `/OCG` in its
    /// `/Properties` was never read. Measured by the #166 security review, returning `Ok` with
    /// poppler and MuPDF hiding the layer. A memo answers "have I done *this* to it", and doing
    /// one thing to an object is not doing the other.
    dictionaries: BTreeSet<(core::ffi::c_int, core::ffi::c_int)>,
    pending: Vec<Pending<O>>,
    deadline: &'d Deadline,
    clock: &'d Arc<dyn Clock>,
}

impl<O: PdfObject> Walk<'_, O> {
    /// `/Annots`: each annotation's own `/OC`, and its appearance streams queued.
    fn annotations(&mut self, page: &O) -> Result<()> {
        let annots = page.key(&ANNOTS);
        if annots.type_code() != object_type::ARRAY {
            return Ok(());
        }
        for at in 0..annots.array_len() {
            self.deadline.checkpoint(self.clock.as_ref())?;
            let annotation = annots.array_item(at);
            // THE TYPE BEFORE THE KEY. `key` on a non-dictionary appends a warning qpdf retains
            // whatever `suppress_warnings` says; the sharing walk measured an `/Annots` of
            // integers growing 330x through exactly that.
            if annotation.type_code() != object_type::DICTIONARY {
                continue;
            }
            if has_oc(&annotation) {
                return refused();
            }
            let appearances = annotation.key(&AP);
            if appearances.type_code() != object_type::DICTIONARY {
                continue;
            }
            for state in &APPEARANCE_STATES {
                let appearance = appearances.key(state);
                match appearance.type_code() {
                    object_type::STREAM => self.queue_stream(appearance)?,
                    // A dictionary of named states, each an appearance stream.
                    object_type::DICTIONARY => {
                        for key in keys_of(&appearance)? {
                            let one = appearance.key(&key);
                            if one.type_code() == object_type::STREAM {
                                self.queue_stream(one)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// One resource dictionary: its optional-content groups, and everything it can draw queued.
    fn resources(&mut self, resources: &O) -> Result<()> {
        // A STREAM HERE NEVER ARRIVES: the sharing walk refuses a stream in a dictionary's place
        // before this runs. See `sharing::Walk::dictionary_key`.
        if resources.type_code() != object_type::DICTIONARY {
            return Ok(());
        }
        // ONCE PER DICTIONARY, like once per stream. A security review measured two layers of N
        // forms sharing one `/Resources`: every form re-read it, adding 2.3 s at N = 2,000.
        let identity = resources.object()?;
        if identity != (0, 0) && !self.dictionaries.insert(identity) {
            return Ok(());
        }
        // `/Properties` is where `BDC /OC /Name` resolves. It also holds ordinary marked-content
        // property lists, so the TYPE decides rather than the category's presence: refusing
        // every page with a `/Properties` entry would refuse tagged documents, which hide
        // nothing.
        let properties = resources.key(&PROPERTIES);
        if properties.type_code() == object_type::DICTIONARY {
            for key in keys_of(&properties)? {
                if is_optional_content_group(&properties.key(&key))? {
                    return refused();
                }
            }
        }
        for category in [&XOBJECT, &PATTERN] {
            let entries = resources.key(category);
            if entries.type_code() != object_type::DICTIONARY {
                continue;
            }
            for key in keys_of(&entries)? {
                let entry = entries.key(&key);
                // A shading pattern is a dictionary and draws no content; an image and a form
                // are both streams, and both may carry `/OC`.
                if entry.type_code() == object_type::STREAM {
                    self.queue_stream(entry)?;
                }
            }
        }
        let fonts = resources.key(&FONT);
        if fonts.type_code() == object_type::DICTIONARY {
            for key in keys_of(&fonts)? {
                let font = fonts.key(&key);
                if font.type_code() == object_type::DICTIONARY && self.first_time(&font)? {
                    self.pending.push(Pending::Font(font));
                }
            }
        }
        Ok(())
    }

    /// A stream's own `/OC`, then its `/Resources`.
    fn stream(&mut self, stream: &O) -> Result<()> {
        if has_oc(&stream.stream_dict()) {
            return refused();
        }
        // AN IMAGE'S DATA IS PIXELS, not operators, and is never read as content.
        if !is_image(&stream.stream_dict())? {
            self.marks(stream)?;
        }
        self.resources(&stream.stream_dict().key(&RESOURCES))
    }

    /// A font's `/Resources`, which only a Type 3 font has and which its procedures draw against.
    fn font(&mut self, font: &O) -> Result<()> {
        self.resources(&font.key(&RESOURCES))
    }

    /// Refuse an `/OC` mark in a stream's own content.
    ///
    /// # Every stream this walk reaches, not only the ones the geometry walk draws
    ///
    /// The geometry walk refuses an `/OC` mark in every stream it draws, and it never draws an
    /// annotation's appearance -- burrow's render leaves annotations off the page -- nor a form
    /// an appearance draws. The #166 security reviews measured both returning `Ok` with a layer
    /// hidden in the output: an inline mark in an appearance, then the same mark one form further
    /// down. So every form, pattern and appearance stream this walk queues is read, not a list of
    /// the ones that happened to be measured.
    ///
    /// # The tag is the second-last operand
    ///
    /// A reader takes a `BDC`'s tag and property list from its last two operands. Reading the
    /// first let `/Pad /OC /OC1 BDC` pass here while PDFium hid the content.
    ///
    /// # Cost, and what is not lexed
    ///
    /// Decoding is the cost, and a stream whose decoded bytes hold no `BDC` at all is not lexed:
    /// it cannot carry a mark, and lexing it could only refuse the page over syntax in content
    /// nobody asked about -- which the third review measured with an inline image in an
    /// appearance elsewhere on the page. A stream that does not decode is skipped: no reader can
    /// draw it either, so nothing in it is shown or hidden.
    fn marks(&mut self, stream: &O) -> Result<()> {
        let Some(content) = stream.stream_data()? else {
            return Ok(());
        };
        if !content.windows(3).any(|window| window == b"BDC") {
            return Ok(());
        }
        for operation in crate::pdfsyntax::ops::operations(&content)? {
            let tag = operation
                .operands
                .len()
                .checked_sub(2)
                .and_then(|at| operation.operands.get(at));
            if operation.operator.as_slice() == b"BDC"
                && matches!(tag,
                    Some(crate::pdfsyntax::ops::Operand::Name { value, .. })
                        if value.as_slice() == b"OC")
            {
                return refused();
            }
        }
        Ok(())
    }

    /// Queue a stream unless it has been queued before.
    fn queue_stream(&mut self, stream: O) -> Result<()> {
        if self.first_time(&stream)? {
            self.pending.push(Pending::Stream(stream));
        }
        Ok(())
    }

    /// Whether `object` is being reached for the first time.
    ///
    /// A **direct** object has no identity — qpdf reports `(0, 0)` — and is always new. That is
    /// safe rather than a hole: a direct object exists inside exactly one parent, and the parent
    /// is what the memo stops re-reading.
    fn first_time(&mut self, object: &O) -> Result<bool> {
        let identity = object.object()?;
        Ok(identity == (0, 0) || self.seen.insert(identity))
    }
}

/// The one refusal, so every site says the same thing.
fn refused() -> Result<()> {
    Err(Error::Unsupported(
        "pdf redaction [optional-content]: this page uses layers (optional content), which can \
         hide text from view, and burrow does not redact a page whose content depends on which \
         layers are shown"
            .to_owned(),
    ))
}

/// Whether a dictionary carries a non-null `/OC`.
fn has_oc<O: PdfObject>(dictionary: &O) -> bool {
    dictionary.type_code() == object_type::DICTIONARY
        && dictionary.key(&OC).type_code() != object_type::NULL
}

/// Whether a stream dictionary is an image XObject's.
fn is_image<O: PdfObject>(dictionary: &O) -> Result<bool> {
    let subtype = dictionary.key(&SUBTYPE);
    if subtype.type_code() != object_type::NAME {
        return Ok(false);
    }
    Ok(subtype.name()? == IMAGE)
}

/// Whether `entry` is an optional-content group or membership dictionary.
fn is_optional_content_group<O: PdfObject>(entry: &O) -> Result<bool> {
    if entry.type_code() != object_type::DICTIONARY {
        return Ok(false);
    }
    let kind = entry.key(&TYPE);
    if kind.type_code() != object_type::NAME {
        return Ok(false);
    }
    let name = kind.name()?;
    Ok(name == OCG || name == OCMD)
}

/// A dictionary's keys, through `unparse` because qpdf's own key iterator may not be called.
fn keys_of<O: PdfObject>(dictionary: &O) -> Result<Vec<Name>> {
    crate::pdfsyntax::dict::top_level_keys(&dictionary.unparse())?
        .iter()
        .map(|key| Name::from_stripped(key))
        .collect()
}
