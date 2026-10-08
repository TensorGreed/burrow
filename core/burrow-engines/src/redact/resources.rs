//! Font and form metrics, read out of a real document.
//!
//! [`pdfsyntax::geometry::Resources`](crate::pdfsyntax::geometry::Resources) is the seam the
//! glyph walk asks for widths, forms and encodings. Everything on this milestone was tested
//! against a **fake** implementation of it that returned the same numbers for every code, and
//! both reviews of #131 found defects that the uniformity hid. This is the real one.
//!
//! # What a real resolver can say that a fake never did
//!
//! The fake always answered. This one refuses, and the refusals are the interesting part:
//!
//! - **A standard-14 font with no `/Widths`** has no metrics *in the document at all*. Helvetica's
//!   advances live in the viewer, not the file. PDFium has them built in and burrow does not, so
//!   the honest answer is a refusal rather than a guess — and a guess here misplaces every glyph
//!   on the page by the difference between the guess and the truth.
//! - **A code outside `/FirstChar`…`/LastChar`** has no width unless `/MissingWidth` is declared.
//! - **A `/FontMatrix` that is not invertible**, a `/W` array that does not parse, a
//!   `/DescendantFonts` that is empty.
//!
//! Each of those is a document the fake would have walked happily.

use std::collections::BTreeMap;

use burrow_types::{Error, Result};

use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::geometry::ScopedFont;
use crate::pdfsyntax::geometry::{
    Encoding, ExtGStateLine, Form, GlyphMetrics, LineParameter, Matrix, Rect, Resources,
};
use crate::redact::graph::PdfObject;

const RESOURCES: Name = Name::literal(b"/Resources\0");
const PARENT: Name = Name::literal(b"/Parent\0");

/// The page-tree climb's ceiling, matching `rotate`'s and `sharing`'s for the same reason: a
/// `/Parent` cycle is writable in any PDF and would otherwise spin this loop forever.
const MAX_PAGE_TREE_DEPTH: u32 = 64;
const XOBJECT: Name = Name::literal(b"/XObject\0");
const SUBTYPE: Name = Name::literal(b"/Subtype\0");
const MATRIX: Name = Name::literal(b"/Matrix\0");
const FONT: Name = Name::literal(b"/Font\0");
const EXT_G_STATE: Name = Name::literal(b"/ExtGState\0");
const LINE_WIDTH: Name = Name::literal(b"/LW\0");
const MITER_LIMIT: Name = Name::literal(b"/ML\0");
const FIRST_CHAR: Name = Name::literal(b"/FirstChar\0");
const WIDTHS: Name = Name::literal(b"/Widths\0");
const FONT_MATRIX: Name = Name::literal(b"/FontMatrix\0");
const FONT_BBOX: Name = Name::literal(b"/FontBBox\0");
const FONT_DESCRIPTOR: Name = Name::literal(b"/FontDescriptor\0");
const MISSING_WIDTH: Name = Name::literal(b"/MissingWidth\0");
const DESCENDANT_FONTS: Name = Name::literal(b"/DescendantFonts\0");
const ENCODING: Name = Name::literal(b"/Encoding\0");
const WMODE: Name = Name::literal(b"/WMode\0");
const DW: Name = Name::literal(b"/DW\0");
const W: Name = Name::literal(b"/W\0");
const BASE_FONT: Name = Name::literal(b"/BaseFont\0");

// The `/Subtype` values this module asks about. Written as `Name`s so the leading slash is
// checked at compile time on both sides of every comparison.
const SUBTYPE_FORM: Name = Name::literal(b"/Form\0");
const SUBTYPE_TYPE0: Name = Name::literal(b"/Type0\0");
const SUBTYPE_TYPE3: Name = Name::literal(b"/Type3\0");

/// The `/Resources` of one stream, resolved against a live document.
pub(crate) struct PageResources<O> {
    /// The resource dictionary itself.
    dictionary: O,
    /// Cached per-font facts, so a page of a thousand glyphs does not re-resolve its font a
    /// thousand times. Keyed by resource name, which is what the content stream selects by.
    fonts: std::cell::RefCell<BTreeMap<Vec<u8>, FontFacts>>,
}

/// Everything the walk needs about one font, read once.
#[derive(Debug, Clone)]
struct FontFacts {
    first_char: i64,
    widths: Vec<f64>,
    missing_width: Option<f64>,
    font_matrix: Matrix,
    font_bbox: Option<Rect>,
    bytes_per_code: u8,
    encoding: Encoding,
    /// `/W` and `/DW` for a CID font, in CID order.
    cid_widths: BTreeMap<u32, f64>,
    default_width: Option<f64>,
    /// What to say when a code has no width, rather than inventing one.
    no_metrics: Option<String>,
    /// The `/BaseFont` name to look a bundled width up by, when and only when the document
    /// declares no `/Widths`. See `read_font`'s precedence comment.
    standard_14: Option<Vec<u8>>,
    /// Which base encoding the font names, for the two codes whose width depends on it.
    base_encoding: crate::pdfsyntax::standard14::BaseEncoding,
}

impl<O: PdfObject> PageResources<O> {
    /// Resolve a page's `/Resources` -- and refuse, rather than climb to, an ancestor's (#224).
    ///
    /// # Inherited resources are refused, since #224
    ///
    /// A page that sets `/Resources` to null is shown without its ancestor's, and burrow cannot
    /// tell that null from an absent key; see the refusal below. The sections that follow
    /// describe the climb this made before, which still decides what "an ancestor's" means: the
    /// nearest `/Pages` node that carries a dictionary.
    ///
    /// # `/Resources` is inheritable, like `/Rotate`
    ///
    /// A page with no `/Resources` of its own uses the nearest ancestor's. `rotate.rs` makes
    /// the same climb for `/Rotate` and `sharing.rs` for this very key; a resolver that stopped
    /// at the page dictionary would report "this page names no fonts" for every document that
    /// puts them on a `/Pages` node — which is how a word processor writes a shared letterhead.
    ///
    /// Note what the committed corpus does **not** exercise: `mixed-rotation-4page.pdf` and
    /// `inherited-rotation-6page.pdf` carry an **empty** `/Resources`, not an absent one. They
    /// refused with `font-missing` before #224, and are refused first now, for the `/Rotate` they
    /// inherit. Absent and empty are different, and only one of them inherits.
    ///
    /// # A `/Parent` chain that does not terminate yields empty resources, not an error
    ///
    /// This section claimed `Error::Malformed` for that case and the function does not raise
    /// one: the climb falls out of its bounded loop and returns the empty dictionary. The
    /// direction is safe today — the resolver then refuses `font-missing` on the first glyph —
    /// but `frame::inherited` *does* refuse in the same situation, so the two differ and only
    /// one of them said so.
    ///
    /// Corrected rather than changed: making this refuse would change what a document with a
    /// long `/Parent` chain does, which is a behaviour question rather than a doc one.
    ///
    /// # Errors
    ///
    /// `Unsupported` `[page-attribute-inherited]` when the page takes its `/Resources` from an
    /// ancestor (#224); whatever qpdf latched while reading the page or an ancestor.
    pub(crate) fn of(owner: &O) -> Result<Self> {
        let direct = owner.key(&RESOURCES);
        if direct.type_code() == object_type::DICTIONARY {
            return Ok(Self {
                dictionary: direct,
                fonts: std::cell::RefCell::new(BTreeMap::new()),
            });
        }
        let mut current = owner.key(&PARENT);
        for _ in 0..MAX_PAGE_TREE_DEPTH {
            if current.type_code() != object_type::DICTIONARY {
                // The top of the tree, or a `/Parent` that is not a node: nothing to inherit,
                // which is not an error. The empty dictionary below then reports every font as
                // missing, which is what a page with no resources genuinely has.
                break;
            }
            let inherited = current.key(&RESOURCES);
            if inherited.type_code() == object_type::DICTIONARY {
                // REFUSED, NOT TAKEN (#224). The page declares no `/Resources` it can read, and
                // one that set it to null would be shown without its ancestor's -- PDFium stops at
                // the null and draws with its own fonts, measured -- while qpdf reads a null as an
                // absent key and climbs to here. burrow cannot see which, so it cannot know the
                // font the text was drawn with, and a review got a zero-width ancestor font to
                // `Ok` with the secret intact. A STOPGAP: #206's second, placement-only reading
                // compares where the glyphs land and would see the difference; ADR 0029 makes it
                // the condition for relaxing this. See `frame::inherited` for why the whole case
                // is refused rather than only the null.
                return Err(Error::Unsupported(
                    "pdf redaction [page-attribute-inherited]: a page that takes its /Resources \
                     from the page tree instead of declaring it. A page that overrides it with \
                     null is shown without it, but burrow cannot tell a null from an absent \
                     entry, so it cannot know which fonts drew the text"
                        .to_owned(),
                ));
            }
            current = current.key(&PARENT);
        }
        Ok(Self {
            dictionary: direct,
            fonts: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    /// The resource dictionary, for a caller that walks it itself.
    pub(crate) const fn dictionary(&self) -> &O {
        &self.dictionary
    }

    fn category(&self, category: &Name) -> O {
        self.dictionary.key(category)
    }

    // `font_object` AND `font_handle` ARE GONE, deliberately.
    //
    // Both took a bare `&[u8]` and searched the page's `/Font`. Five consumers used them on a
    // name read inside a Form XObject, which is a different scope, and each was a defect: two
    // leaks and three refusals of documents burrow handles. Four review rounds found four of
    // them, one at a time.
    //
    // Deleting them is what makes [`Self::font_in_scope`] the only answer. A `pub(crate)`
    // page-scoped resolver left beside it is a resolver somebody reaches for, and the whole
    // argument of `ScopedFont` is that the wrong scope should stop compiling rather than be
    // found a sixth time.

    /// The font dictionary a glyph's [`ScopedFont`] selects, **in the stream that named it**.
    ///
    /// Along the glyph's route, each form's own `/Resources` or the enclosing ones where it
    /// declares none -- the inheritance `Resources::within` applies, so this resolves a name the
    /// way the walk that read it did. Anything that does not is a bypass by construction;
    /// [`ScopedFont`] carries the five times that mattered, and the sixth (#221).
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the route does not resolve to it. The walk already placed a glyph
    /// through it, so this is burrow disagreeing with itself rather than the document being
    /// wrong — and a silent `None` would be the narrowing every leak here has been.
    pub(crate) fn font_in_scope(&self, font: &ScopedFont) -> Result<O> {
        self.font_path_in_scope(font).map(|(found, _)| found)
    }

    /// [`Self::font_in_scope`], and the path it was found by (#218, #221): the route of `Do`
    /// names the walk followed from the page to the stream that named the font, and the font's
    /// name.
    ///
    /// # The walk's route, followed -- never searched for
    ///
    /// The path is the route the geometry walk recorded on the glyph ([`ScopedFont::route`]),
    /// resolved step by step through the scope in force at each `Do`. Two earlier versions
    /// reconstructed it instead, and both credited a glyph to a font it was not drawn with: the
    /// first looked for the form among the page's top-level `/XObject` only and fell back to the
    /// page's font of the same name (#221); the second searched for the form by id and took the
    /// first route found, which is wrong for a form drawn from two scopes, and whose answer
    /// depended on how qpdf sorted the names. Each returned `Ok` with a removed code still
    /// mapped. A route recorded by the walk cannot disagree with the walk.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the route does not resolve to a font dictionary in these
    /// resources. The walk drew a glyph along it, so that is burrow disagreeing with itself.
    pub(crate) fn font_path_in_scope(
        &self,
        font: &ScopedFont,
    ) -> Result<(O, crate::redact_verify::FontPath)> {
        let path = crate::redact_verify::FontPath {
            form: font.route().to_vec(),
            name: font.name().to_vec(),
        };
        let found = self.resolve(&path)?.ok_or_else(|| {
            Error::Internal(
                "pdf resources: a font the walk drew a glyph with does not resolve along the \
                 route the walk took"
                    .to_owned(),
            )
        })?;
        Ok((found, path))
    }

    /// The font dictionary `path` names, or `None`: each chain entry through the `/XObject` of
    /// the scope in force, stepping into a form's own `/Resources` where it declares them.
    fn resolve(&self, path: &crate::redact_verify::FontPath) -> Result<Option<O>> {
        resolve_from(&self.dictionary, &path.form, &path.name)
    }

    /// The font dictionary a [`crate::redact_verify::FontPath`] names, for the read-back (#218).
    ///
    /// # Errors
    ///
    /// [`Error::OutputRejected`] when the path does not reach a font dictionary: a font the
    /// operation cut that the output does not have where it was cut is a check that cannot find
    /// its subject, and that refuses.
    pub(crate) fn font_at(&self, path: &crate::redact_verify::FontPath) -> Result<O> {
        self.resolve(path)?.ok_or_else(|| {
            Error::OutputRejected(
                "redact: the region is not cleared -- a font the operation cut is not where it \
                 was cut in the output"
                    .to_owned(),
            )
        })
    }

    fn font_facts(&self, name: &[u8]) -> Result<FontFacts> {
        if let Some(cached) = self.fonts.borrow().get(name) {
            return Ok(cached.clone());
        }
        let key = Name::from_stripped(name)?;
        let font = self.category(&FONT).key(&key);
        if font.type_code() != object_type::DICTIONARY {
            return Err(Error::Malformed(format!(
                "pdf resources [font-missing]: the content stream selects a font the page's \
                 resources do not name ({} bytes)",
                name.len()
            )));
        }
        let facts = read_font(&font)?;
        self.fonts.borrow_mut().insert(name.to_vec(), facts.clone());
        Ok(facts)
    }
}

impl<O: PdfObject> Resources for PageResources<O> {
    fn ext_gstate_sets_font(&self, name: &[u8]) -> Result<bool> {
        let key = Name::from_stripped(name)?;
        // THE CATEGORY'S TYPE FIRST: `key` on anything but a dictionary makes qpdf retain a
        // warning per call, and a page of a million `gs` reached 2.2 GB that way (#152's
        // security review). The sharing walk refuses such a category before this runs; this
        // keeps the cost off even where it did not.
        let category = self.category(&EXT_G_STATE);
        if category.type_code() != object_type::DICTIONARY {
            return Ok(false);
        }
        let Some(state) = state_dictionary(category.key(&key)) else {
            return Ok(false);
        };
        // PRESENT, WHATEVER ITS VALUE, and that is the conservative reading rather than the
        // measured one: PDFium drops the `Tf` font for a `/Font [font size]` array, measured even
        // where it names the `Tf`'s own object (#152), and ignores a bare font reference there.
        // Refusing both is the safe direction; the census found no `/Font` array in 224 local
        // documents, and did not count bare references, so their cost is unmeasured. This scope only: the walk asks the enclosing
        // scopes too, because PDFium falls back to the page's `/ExtGState` when a form has none
        // (`geometry::ScopeChain`).
        Ok(state.key(&FONT).type_code() != object_type::NULL)
    }

    fn ext_gstate_line(&self, name: &[u8]) -> Result<ExtGStateLine> {
        let key = Name::from_stripped(name)?;
        // THE CATEGORY'S TYPE FIRST, for `ext_gstate_sets_font`'s reason: `key` on anything but a
        // dictionary costs a retained qpdf warning per call.
        let category = self.category(&EXT_G_STATE);
        if category.type_code() != object_type::DICTIONARY {
            return Ok(ExtGStateLine::UNSET);
        }
        let Some(state) = state_dictionary(category.key(&key)) else {
            return Ok(ExtGStateLine::UNSET);
        };
        Ok(ExtGStateLine {
            width: line_parameter(&state.key(&LINE_WIDTH)),
            miter: line_parameter(&state.key(&MITER_LIMIT)),
        })
    }

    fn within(&self, name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>> {
        let key = Name::from_stripped(name)?;
        let entry = self.category(&XOBJECT).key(&key);
        if entry.type_code() != object_type::STREAM {
            return Ok(None);
        }
        let own = entry.stream_dict().key(&RESOURCES);
        if own.type_code() != object_type::DICTIONARY {
            // THE FORM DECLARES NONE, so it inherits the enclosing ones. `None` says that
            // rather than returning an empty dictionary, because an empty one would refuse
            // every name the form uses instead of resolving it outwards.
            return Ok(None);
        }
        Ok(Some(Box::new(Self {
            dictionary: own,
            fonts: std::cell::RefCell::new(BTreeMap::new()),
        })))
    }

    fn form(&self, name: &[u8]) -> Result<Option<Form>> {
        let key = Name::from_stripped(name)?;
        let entry = self.category(&XOBJECT).key(&key);
        match entry.type_code() {
            object_type::STREAM => {}
            // A NAME THE RESOURCES DO NOT HOLD IS REFUSED, not read as "draws nothing". Readers
            // disagree about what it draws, and one of them draws text: PDFium falls back to
            // the page's `/XObject` when a form's own resources lack it, which a security review
            // of #166 measured drawing `SECRETWORD` that burrow's walk skipped. And qpdf repairs
            // an invalid inherited `/Resources` by giving the page an empty one, so a `Do` that
            // PDFium resolves through the original arrives here naming nothing. A walk that
            // steps over it places no glyph where the reader draws some, and the redaction
            // removes nothing and returns `Ok`.
            object_type::NULL => {
                return Err(Error::Malformed(
                    "pdf resources [xobject-missing]: the content draws an XObject its \
                     resources do not name, which readers resolve differently"
                        .to_owned(),
                ));
            }
            // Present and not a stream: not an XObject any reader draws.
            _ => return Ok(None),
        }
        let dictionary = entry.stream_dict();
        if subtype_of(&dictionary)? != Some(SUBTYPE_FORM) {
            return Ok(None);
        }
        let Some(content) = entry.stream_data()? else {
            return Err(Error::Malformed(
                "pdf resources [form-unreadable]: a Form XObject whose data burrow could not \
                 decode, so what it draws is unknown"
                    .to_owned(),
            ));
        };
        let matrix = numbers_strict(&dictionary.key(&MATRIX))?.unwrap_or_default();
        let matrix = match matrix.as_slice() {
            [a, b, c, d, e, f] => Matrix {
                a: *a,
                b: *b,
                c: *c,
                d: *d,
                e: *e,
                f: *f,
            },
            // `/Matrix` is optional and defaults to the identity.
            [] => Matrix::IDENTITY,
            _ => {
                return Err(Error::Malformed(
                    "pdf resources [form-matrix]: a Form XObject `/Matrix` that is not six \
                     numbers"
                        .to_owned(),
                ));
            }
        };
        let (number, generation) = entry.object()?;
        Ok(Some(Form {
            // Object identity, packed. `core/CLAUDE.md`: a `qpdf_oh` handle is not an identity;
            // the object number and generation are.
            id: (u64::from(number.unsigned_abs()) << 16) | u64::from(generation.unsigned_abs()),
            matrix,
            content,
        }))
    }

    fn glyph(&self, name: &[u8], code: u32) -> Result<GlyphMetrics> {
        let facts = self.font_facts(name)?;
        let width = width_of(&facts, code).ok_or_else(|| {
            Error::Malformed(facts.no_metrics.clone().unwrap_or_else(|| {
                "pdf resources [no-width]: a code the font declares no width for, and no \
                     `/MissingWidth` to fall back on"
                    .to_owned()
            }))
        })?;
        Ok(GlyphMetrics {
            width,
            bytes_per_code: facts.bytes_per_code,
            font_bbox: facts.font_bbox,
            font_matrix: facts.font_matrix,
            encoding: facts.encoding.clone(),
        })
    }

    fn bytes_per_code(&self, name: &[u8]) -> Result<u8> {
        Ok(self.font_facts(name)?.bytes_per_code)
    }
}

/// The width for `code`, or `None` when neither the document nor the bundled tables have one.
///
/// # The order is the precedence, and it is the document first
///
/// `/W` and `/DW` for a CID font; then the document's own `/Widths` indexed from `/FirstChar`;
/// then `/MissingWidth`; and only then the bundled standard-14 table — which `read_font` fills
/// **only when `/Widths` is empty**, so a font that declares its own widths cannot reach it.
///
/// A document may declare widths that differ from the published metrics, and it is entitled to:
/// drawing with the table would then place every glyph where the file does not.
fn width_of(facts: &FontFacts, code: u32) -> Option<f64> {
    if facts.bytes_per_code > 1 {
        return facts.cid_widths.get(&code).copied().or(facts.default_width);
    }
    let index = i64::from(code) - facts.first_char;
    let declared = usize::try_from(index)
        .ok()
        .and_then(|at| facts.widths.get(at).copied())
        .or(facts.missing_width);
    if declared.is_some() {
        return declared;
    }
    let base = facts.standard_14.as_deref()?;
    crate::pdfsyntax::standard14::width_of(base, code, facts.base_encoding)
}

/// Which base encoding a simple font's `/Encoding` names.
///
/// A name, or the `/BaseEncoding` inside an `/Encoding` dictionary. Anything else — including a
/// dictionary with only `/Differences` — is `Standard`, which is what PDF 32000-1 §9.6.6.1 says
/// a font with no stated base encoding uses for a non-symbolic font.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `base-encoding-not-a-name` for a `/BaseEncoding` that is present
/// and not a name (#125's third security review). PDFium reads it by its bytes, so
/// `(WinAnsiEncoding)` is WinAnsi to it and was Standard here: 50 codes of 96 then SECRET, placed
/// with Standard's widths, sat outside the region and stayed -- `Ok`, 385 dark pixels before and
/// after. A top-level `/Encoding` string is not honoured by PDFium, and is Standard in both.
fn base_encoding<O: PdfObject>(font: &O) -> Result<crate::pdfsyntax::standard14::BaseEncoding> {
    use crate::pdfsyntax::standard14::BaseEncoding;
    const BASE_ENCODING: Name = Name::literal(b"/BaseEncoding\0");
    const WIN_ANSI: Name = Name::literal(b"/WinAnsiEncoding\0");

    let encoding = font.key(&ENCODING);
    Ok(match encoding.type_code() {
        object_type::NAME => {
            if names(&encoding, &WIN_ANSI) {
                BaseEncoding::WinAnsi
            } else {
                BaseEncoding::Standard
            }
        }
        object_type::DICTIONARY => {
            let base = encoding.key(&BASE_ENCODING);
            match base.type_code() {
                object_type::NULL => BaseEncoding::Standard,
                object_type::NAME if names(&base, &WIN_ANSI) => BaseEncoding::WinAnsi,
                object_type::NAME => BaseEncoding::Standard,
                _ => {
                    return Err(Error::Unsupported(
                        "pdf resources [base-encoding-not-a-name]: an /Encoding whose \
                         /BaseEncoding is not a name, which a renderer reads by its bytes"
                            .to_owned(),
                    ));
                }
            }
        }
        _ => BaseEncoding::Standard,
    })
}

/// Read one font dictionary.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `subtype-not-a-name`, `number-unreadable` (any of the number
/// keys), or `base-encoding-not-a-name`; [`Error::Malformed`] naming `type3-matrix`; and whatever
/// [`read_composite`] refuses for a Type 0 font.
fn read_font<O: PdfObject>(font: &O) -> Result<FontFacts> {
    let subtype = subtype_of(font)?;
    let mut facts = FontFacts {
        first_char: integer_or(&font.key(&FIRST_CHAR), 0)?,
        widths: numbers_strict_up_to(&font.key(&WIDTHS), SIMPLE_FONT_CODES)?.unwrap_or_default(),
        missing_width: None,
        // 0.001 for every font but Type 3, which declares its own.
        font_matrix: Matrix::scale(0.001, 0.001),
        font_bbox: rect_of(&font.key(&FONT_BBOX))?,
        bytes_per_code: 1,
        encoding: Encoding::Simple,
        cid_widths: BTreeMap::new(),
        default_width: None,
        no_metrics: None,
        standard_14: None,
        base_encoding: crate::pdfsyntax::standard14::BaseEncoding::Standard,
    };

    let descriptor = font.key(&FONT_DESCRIPTOR);
    if descriptor.type_code() == object_type::DICTIONARY {
        facts.missing_width = number_strict(&descriptor.key(&MISSING_WIDTH))?;
    }

    if subtype == Some(SUBTYPE_TYPE3) {
        match numbers_strict(&font.key(&FONT_MATRIX))?
            .unwrap_or_default()
            .as_slice()
        {
            [a, b, c, d, e, f] => {
                facts.font_matrix = Matrix {
                    a: *a,
                    b: *b,
                    c: *c,
                    d: *d,
                    e: *e,
                    f: *f,
                };
            }
            _ => {
                return Err(Error::Malformed(
                    "pdf resources [type3-matrix]: a Type 3 font with no usable `/FontMatrix`, \
                     which is the only thing that says how big its glyphs are"
                        .to_owned(),
                ));
            }
        }
    }

    if subtype == Some(SUBTYPE_TYPE0) {
        read_composite(font, &mut facts)?;
    } else if facts.widths.is_empty() {
        // NO METRICS IN THE DOCUMENT. A standard-14 font's advances live in the viewer rather
        // than the file, and burrow now bundles them -- see `pdfsyntax::standard14` for the
        // provenance and for what is deliberately not tabulated.
        //
        // **THIS BRANCH IS THE PRECEDENCE.** It is reached only when `/Widths` is empty, so a
        // font that declares its own widths never consults the table: the document's numbers
        // win over the bundled ones, always. That is not a preference -- a document may declare
        // widths that differ from the published metrics, and drawing with the table would then
        // place every glyph where the file does not.
        let base = font.key(&BASE_FONT).name();
        facts.standard_14 = base.as_ref().ok().map(|name| name.plain().to_vec());
        facts.base_encoding = base_encoding(font)?;
        // SET EVEN WHEN A TABLE IS FOUND, because the table may not carry the particular code
        // the page draws -- an untabulated font, a code outside 32..=126, or one of the pairs
        // the calibration found the two sources disagreeing on. `width_of` falls back to this
        // message whenever the lookup comes back empty, so the reason a page refuses is the
        // informative one rather than the generic "no width for this code".
        facts.no_metrics = Some(format!(
            "pdf resources [no-widths]: a font with no `/Widths` array whose `/BaseFont` \
             ({} bytes) is not one burrow carries metrics for, at least not for every code \
             this page draws -- see `pdfsyntax::standard14` for what is carried and why",
            base.map_or(0, |name| name.plain().len())
        ));
    }
    Ok(facts)
}

/// A Type 0 font: the descendant's `/W` and `/DW`, and the `/Encoding` CMap.
///
/// # Errors
///
/// [`Error::Malformed`] naming `descendant-fonts`; [`Error::Unsupported`] naming
/// `number-unreadable` for a `/DW` or `/WMode` that is not a number, and whatever [`parse_w`]
/// refuses.
fn read_composite<O: PdfObject>(font: &O, facts: &mut FontFacts) -> Result<()> {
    facts.bytes_per_code = 2;

    let encoding = font.key(&ENCODING);
    facts.encoding = match encoding.type_code() {
        object_type::NAME => match encoding.name() {
            // `Encoding::Predefined` wants the registry name WITHOUT its slash, which is how
            // `predefined_writing_mode` matches the `-H`/`-V` convention.
            Ok(name) => Encoding::Predefined(name.plain().to_vec()),
            Err(_) => Encoding::UnreadableCMap,
        },
        object_type::STREAM => {
            let dictionary = encoding.stream_dict();
            let wmode = number_strict(&dictionary.key(&WMODE))?.and_then(|value| {
                // `as` TRUNCATES SILENTLY and this crate denies it. A `/WMode` that is not
                // a whole small number is not a writing mode; refusing to read one is the
                // conservative direction, and `writing_mode_of` refuses it by name.
                whole(value)
            });
            match encoding.stream_data()? {
                Some(program) => Encoding::Embedded {
                    dictionary_wmode: wmode,
                    program,
                },
                // THE RESOLVER COULD NOT READ IT, and says so rather than defaulting. See
                // `Encoding::UnreadableCMap`: horizontal-by-omission is the leak direction.
                None => Encoding::UnreadableCMap,
            }
        }
        _ => Encoding::UnreadableCMap,
    };

    let descendants = font.key(&DESCENDANT_FONTS);
    if descendants.type_code() != object_type::ARRAY || descendants.array_len() < 1 {
        return Err(Error::Malformed(
            "pdf resources [descendant-fonts]: a Type 0 font with no descendant, so its widths \
             are nowhere"
                .to_owned(),
        ));
    }
    let descendant = descendants.array_item(0);
    facts.default_width = number_strict(&descendant.key(&DW))?.or(Some(1000.0));
    facts.cid_widths = parse_w(&descendant.key(&W))?;
    Ok(())
}

/// The most codes a `/W` array may assign, counting a code each time it is assigned.
///
/// A CID is at most 65,535 (PDF 32000-1 Annex C), so a real `/W` assigns at most 65,536 codes
/// and overlaps are rare. Twice that is room for a careless producer and nowhere near the
/// attack: each `cFirst cLast w` triple assigns up to 65,536 codes from about a dozen bytes,
/// and nothing counted the triples. A 1.2 MB file repeating `0 65535 500` a hundred thousand
/// times took **175 s** here, against any budget -- this runs inside the glyph walk, between the
/// deadline's reads.
pub(crate) const MAX_W_ASSIGNMENTS: usize = 131_072;

/// `/W`, in either of its two shapes: `c [w …]` and `cFirst cLast w`.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `widths-too-many` once the array assigns more than
/// [`MAX_W_ASSIGNMENTS`] codes -- or takes that many steps, counting each entry as one so an array
/// of empty runs cannot walk unbounded -- and `number-unreadable` for an item where a number
/// belongs that is not one. A truncated tail stops where it ends. Refused, not truncated: a width map that stopped growing places
/// the rest of the font's glyphs with the default width, which is a box in the wrong place.
fn parse_w<O: PdfObject>(array: &O) -> Result<BTreeMap<u32, f64>> {
    let mut widths = BTreeMap::new();
    if array.type_code() != object_type::ARRAY {
        return Ok(widths);
    }
    let mut assigned: usize = 0;
    let mut assign = |widths: &mut BTreeMap<u32, f64>, code: u32, width: f64| -> Result<()> {
        assigned += 1;
        if assigned > MAX_W_ASSIGNMENTS {
            return Err(Error::Unsupported(
                "pdf resources [widths-too-many]: a CID font's /W assigns more widths than \
                 there are CIDs to assign them to"
                    .to_owned(),
            ));
        }
        // THE FIRST ENTRY FOR A CODE IS THE ONE PDFIUM USES (#125's fourth security review):
        // `LoadMetricsArray` stops at the first match, and this map kept the last -- `[5 [100] 5
        // [3000]]` put SECRET 180 points from where PDFium drew it, `Ok`. Refused rather than
        // modelled: 0 of 99 real documents overlap.
        if widths.contains_key(&code) {
            return Err(Error::Unsupported(
                "pdf resources [widths-overlap]: a CID font's /W gives one code two widths, and \
                 renderers disagree on which one counts"
                    .to_owned(),
            ));
        }
        widths.insert(code, width);
        Ok(())
    };
    let length = array.array_len();
    // ITEM BY ITEM, AND A NON-NUMBER REFUSES (#125's third security review): through `numbers_of`
    // an inner `6 0 R` read as two numbers and a string vanished, shifting every later width. An
    // array that ENDS mid-entry still stops where it ends, as before (#224, round 3).
    let mut at = 0;
    let mut steps: usize = 0;
    while at < length {
        steps += 1;
        if steps > MAX_W_ASSIGNMENTS {
            return Err(Error::Unsupported(
                "pdf resources [widths-too-many]: a CID font's /W assigns more widths than \
                 there are CIDs to assign them to"
                    .to_owned(),
            ));
        }
        let Some(start) = number_strict(&array.array_item(at))? else {
            return Err(number_unreadable());
        };
        if at + 1 >= length {
            break;
        }
        let next = array.array_item(at + 1);
        if next.type_code() == object_type::ARRAY {
            let widths_here = numbers_strict(&next)?.unwrap_or_default();
            // A START THAT IS NOT WHOLE REFUSES: it was skipped, where PDFium truncates `5.5` to 5
            // and reads on (#125's fourth security review).
            let base = whole(start).ok_or_else(number_unreadable)?;
            for (offset, width) in widths_here.into_iter().enumerate() {
                let Ok(offset) = i64::try_from(offset) else {
                    break;
                };
                if let Ok(code) = u32::try_from(base.saturating_add(offset)) {
                    assign(&mut widths, code, width)?;
                }
            }
            at += 2;
        } else {
            let Some(end) = number_strict(&next)? else {
                return Err(number_unreadable());
            };
            if at + 2 >= length {
                break;
            }
            let Some(width) = number_strict(&array.array_item(at + 2))? else {
                return Err(number_unreadable());
            };
            // NOT WHOLE REFUSES, where it used to stop the whole array: PDFium truncates `0.5` to 0
            // and reads every entry after it (#125's fourth security review).
            let start = whole(start).ok_or_else(number_unreadable)?;
            let end = whole(end).ok_or_else(number_unreadable)?;
            // A RANGE THE FILE CHOOSES. Bounded so a `/W` of `[0 4294967295 500]` does not
            // materialise four billion entries in a map -- from the first code that exists, so a
            // negative start does not spend the bound on codes no font has.
            let start = start.max(0);
            for code in start..=end.min(start + 65_535) {
                if let Ok(code) = u32::try_from(code) {
                    assign(&mut widths, code, width)?;
                }
            }
            at += 3;
        }
    }
    Ok(widths)
}

/// Whether a name-valued handle names `want`.
///
/// # The slash is on the read side too, and the type now says so
///
/// `PdfObject::name` returns a [`Name`], which carries its leading `/` by construction, and
/// `want` is a `Name::literal` whose missing slash would be a **compile error**. So the
/// mismatch is no longer expressible.
///
/// It was live before that: every `/Subtype` check here compared a slashed name against a bare
/// byte string, so a Type 0 font read as a simple one, took the no-`/Widths` path, and the OCR
/// fixture refused with entirely the wrong rule. The fake `Resources` never called `name()`, so
/// nothing on this milestone could have caught it.
///
/// A handle that is not a name is not the name asked about, which is `false` rather than an
/// error: the callers are all "is this a Type 3 font" questions where absent means no.
fn names<O: PdfObject>(handle: &O, want: &Name) -> bool {
    handle.name().is_ok_and(|found| found == *want)
}

/// The `/Subtype` burrow may branch on: absent, or a name -- and a refusal for anything else (#125).
///
/// PDFium reads `/Subtype` as its BYTES, so `(Type3)` is a Type 3 font and `(Form)` a form to it
/// (#229 measured the same for annotations). Comparing a name and finding none read a string as
/// "not this", so the walk skipped a form's content and every Type 3 rule skipped the font: the
/// security review of #125's Type 3 slice measured `Ok` with a procedure's painted path, its image
/// and its shown text all left in the region, and a form's text likewise -- 879 dark pixels
/// before and after. Refused rather than read as bytes (DECISIONS.md rule 1): one reader of a
/// malformed key is not the other's.
///
/// # Errors
///
/// [`Error::Unsupported`] for a `/Subtype` that is present and not a name.
pub(super) fn subtype_of<O: PdfObject>(dictionary: &O) -> Result<Option<Name>> {
    let subtype = dictionary.key(&SUBTYPE);
    match subtype.type_code() {
        object_type::NULL => Ok(None),
        object_type::NAME => Ok(Some(subtype.name()?)),
        _ => Err(Error::Unsupported(
            "pdf redaction [subtype-not-a-name]: a /Subtype that is not a name, which a renderer \
             reads by its bytes and burrow would read as absent"
                .to_owned(),
        )),
    }
}

/// [`PageResources::resolve`]'s walk from one scope along `steps`.
///
/// Each step is looked up in the `/XObject` of the scope in force, and the next scope is that
/// form's own `/Resources` when it is a dictionary and the enclosing one otherwise -- the rule
/// `Resources::within` applies, so a route the walk recorded resolves as the walk resolved it.
///
/// # Errors
///
/// Only what reading a name from the document can raise. A step or a font that is not there is
/// `Ok(None)`, and each caller decides what that means: `Internal` for a route the walk drew
/// along, `OutputRejected` for a cut font the read-back cannot find.
fn resolve_from<O: PdfObject>(scope: &O, steps: &[Vec<u8>], name: &[u8]) -> Result<Option<O>> {
    const RESOURCES: Name = Name::literal(b"/Resources\0");
    let Some((step, rest)) = steps.split_first() else {
        let found = scope.key(&FONT).key(&Name::from_stripped(name)?);
        return Ok((found.type_code() == object_type::DICTIONARY).then_some(found));
    };
    let entry = scope.key(&XOBJECT).key(&Name::from_stripped(step)?);
    if entry.type_code() != object_type::STREAM {
        return Ok(None);
    }
    let own = entry.stream_dict().key(&RESOURCES);
    if own.type_code() == object_type::DICTIONARY {
        resolve_from(&own, rest, name)
    } else {
        resolve_from(scope, rest, name)
    }
}

/// A `f64` that is exactly a whole number, as an `i64`.
///
/// `as` truncates silently and this crate denies it: a `/FirstChar` of `65.7` is not a
/// character code, and rounding it to 65 would place every glyph in the font by one code.
pub(crate) fn whole(value: f64) -> Option<i64> {
    (value.is_finite() && value.fract() == 0.0 && value.abs() < 9e15).then(|| {
        // Exact: the guard above establishes the value is integral and inside i64.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "guarded: finite, integral, and inside i64's range"
        )]
        let converted = value as i64;
        converted
    })
}

/// The codes a simple font has: one byte each. `/Widths` is read no further, as PDFium reads it no
/// further, so a 65,536-item `/Widths` costs 256 reads, and an item past code 255 is never judged
/// (#125's fourth code review).
const SIMPLE_FONT_CODES: std::ffi::c_int = 256;

/// The most items [`numbers_strict`] reads from one array. A simple font's `/Widths` has at most
/// 256, a matrix six, a box four; a CID font's `/W` inner array is the long one, and `parse_w`
/// bounds what it assigns separately.
const MAX_NUMBER_ARRAY: std::ffi::c_int = 65_536;

/// The refusal for a number-valued key holding something [`numbers_strict`] will not read.
fn number_unreadable() -> Error {
    Error::Unsupported(
        "pdf resources [number-unreadable]: a number, or an array of numbers, holding something \
         else -- a string, a name, an array, a number past the range both readers hold -- which \
         a renderer reads differently"
            .to_owned(),
    )
}

/// A number-valued key, or an array of numbers, read ITEM BY ITEM as PDFium reads it (#125's
/// third security review). `None` when the key is absent.
///
/// The reader it replaced tokenised the array's unparsed text, which resolves only the array itself: an
/// inner `6 0 R` came out as the two numbers 6 and 0, and a string or a name was dropped, so every
/// later item shifted. PDFium reads each item, resolving a reference, and reads a matrix that is
/// not six numbers as the identity. Measured: `/Widths [6 0 R 600 …]` placed SECRET's glyphs
/// where the region was not and returned `Ok` with all of it on the page; a form `/Matrix` with a
/// string in it, and a Type 3 `/FontMatrix` likewise, `Ok` over 374 and 7,500 dark pixels. Each
/// item here is one integer or real both readers PARSE alike ([`super::frame::reading_of`]) -- a
/// reference resolves to its number, as PDFium resolves it -- and anything else refuses:
/// DECISIONS.md rule 1, one reader's repair is not the other's.
///
/// **Parse alike, not use alike.** PDFium stores a simple or CID font's widths as integers,
/// truncating a fraction, and rounds a Type 3 font's; this keeps the fraction. **That leaks**:
/// over enough padding glyphs the drift carries the secret past the region's edge while PDFium
/// still draws it inside -- measured by #125's fourth security review for all three font kinds.
/// #291, a ship blocker. (A first probe with 0.99 over 4,000 glyphs found no leak; it was the wrong
/// shape, and the claim it supported is withdrawn.)
///
/// Also refused, and new with this reader: a number past `reading_of`'s range (2^24, where a
/// 32-bit float stops holding every whole number), and an array longer than [`MAX_NUMBER_ARRAY`].
///
/// # Errors
///
/// [`Error::Unsupported`] naming `number-unreadable` for a value that is neither a number nor an
/// array, an item that is not a number, a number past `reading_of`'s range, or an array longer
/// than [`MAX_NUMBER_ARRAY`].
fn numbers_strict<O: PdfObject>(handle: &O) -> Result<Option<Vec<f64>>> {
    use super::frame::{Reading, reading_of};
    match handle.type_code() {
        object_type::NULL => Ok(None),
        object_type::INTEGER | object_type::REAL => match reading_of(handle) {
            Reading::Number(value) => Ok(Some(vec![value])),
            _ => Err(number_unreadable()),
        },
        object_type::ARRAY => {
            let length = handle.array_len();
            if length > MAX_NUMBER_ARRAY {
                return Err(number_unreadable());
            }
            let mut values = Vec::new();
            for at in 0..length {
                match reading_of(&handle.array_item(at)) {
                    Reading::Number(value) => values.push(value),
                    _ => return Err(number_unreadable()),
                }
            }
            Ok(Some(values))
        }
        _ => Err(number_unreadable()),
    }
}

/// As [`numbers_strict`], reading at most the first `up_to` items of an array.
///
/// # Errors
///
/// As [`numbers_strict`], for the items read.
fn numbers_strict_up_to<O: PdfObject>(
    handle: &O,
    up_to: std::ffi::c_int,
) -> Result<Option<Vec<f64>>> {
    use super::frame::{Reading, reading_of};
    if handle.type_code() != object_type::ARRAY {
        return numbers_strict(handle);
    }
    let mut values = Vec::new();
    for at in 0..handle.array_len().min(up_to) {
        match reading_of(&handle.array_item(at)) {
            Reading::Number(value) => values.push(value),
            _ => return Err(number_unreadable()),
        }
    }
    Ok(Some(values))
}

/// One number-valued key, strictly: `None` when absent, the number when it is one, and a refusal
/// for anything else -- an array included, which PDFium reads as 0 where a number belongs.
///
/// # Errors
///
/// As [`numbers_strict`], and for an array.
fn number_strict<O: PdfObject>(handle: &O) -> Result<Option<f64>> {
    if handle.type_code() == object_type::ARRAY {
        return Err(number_unreadable());
    }
    Ok(numbers_strict(handle)?.and_then(|values| values.first().copied()))
}

/// An integer-valued key with a fallback for absent, refusing a value that is not a number.
///
/// # Errors
///
/// As [`number_strict`].
fn integer_or<O: PdfObject>(handle: &O, fallback: i64) -> Result<i64> {
    // A NUMBER THAT IS NOT WHOLE REFUSES (#125's fourth code review): `/FirstChar 65.5` fell back to
    // 0 here while PDFium truncates it to 65, so every width sat 65 codes out. `steps::first_char`
    // refuses it too, but only for a font that is cut -- and a font misplaced off the region is not.
    match number_strict(handle)? {
        None => Ok(fallback),
        Some(value) => whole(value).ok_or_else(number_unreadable),
    }
}

/// A box: `None` when absent or not four numbers, and a refusal when an item is not a number.
///
/// # Errors
///
/// As [`numbers_strict`].
pub(super) fn rect_of<O: PdfObject>(handle: &O) -> Result<Option<Rect>> {
    Ok(match numbers_strict(handle)?.as_deref() {
        Some([left, bottom, right, top]) => Some(Rect {
            left: left.min(*right),
            bottom: bottom.min(*top),
            right: left.max(*right),
            top: bottom.max(*top),
        }),
        _ => None,
    })
}

/// The dictionary an ExtGState entry's keys are read from: the entry itself, or **a stream's own
/// dictionary** when the entry is written as a stream (#278's code review).
///
/// PDFium and poppler ignore a stream entry -- measured for PDFium by #278's specification review,
/// a stream whose dictionary held `/LW 120` drew a thin line -- and a reader that resolves keys
/// through a stream's dictionary would apply it. Which one draws is reader-dependent, so it is
/// read as though it applied (DECISIONS.md rule 1): its `/Font` refuses and its `/LW` widens the
/// box, an over-refusal for PDFium and the safe direction for the rest. Anything else is no
/// state at all.
fn state_dictionary<O: PdfObject>(entry: O) -> Option<O> {
    match entry.type_code() {
        object_type::DICTIONARY => Some(entry),
        object_type::STREAM => Some(entry.stream_dict()),
        _ => None,
    }
}

/// One `/LW` or `/ML` value as both readers would take it (#278).
///
/// Absent (or `null`) is unset; one integer or real both readers agree on, within the page frame's
/// magnitude bound, is that value -- an indirect reference to one included, since qpdf resolves it
/// and PDFium applies it (#278's review measured 7,128 dark pixels from `/LW 6 0 R`); anything else
/// is unreadable. A string or array draws thin in PDFium, so refusing it over-refuses: recorded,
/// and taken rather than guessing what another reader makes of it.
fn line_parameter<O: PdfObject>(value: &O) -> LineParameter {
    if value.type_code() == object_type::NULL {
        return LineParameter::UNSET;
    }
    match super::frame::reading_of(value) {
        super::frame::Reading::Number(number) => LineParameter::set(number),
        super::frame::Reading::NotANumber | super::frame::Reading::OutOfRange => {
            LineParameter::UNREADABLE
        }
    }
}
