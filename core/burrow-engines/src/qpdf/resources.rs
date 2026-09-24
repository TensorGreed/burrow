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

use super::handle::ObjectHandle;
use super::name::Name;
use crate::codes::qpdf::object_type;
use crate::pdfsyntax::geometry::ScopedFont;
use crate::pdfsyntax::geometry::{Encoding, Form, GlyphMetrics, Matrix, Rect, Resources};

const RESOURCES: Name = Name::literal(b"/Resources\0");
const PARENT: Name = Name::literal(b"/Parent\0");

/// The page-tree climb's ceiling, matching `rotate`'s and `sharing`'s for the same reason: a
/// `/Parent` cycle is writable in any PDF and would otherwise spin this loop forever.
const MAX_PAGE_TREE_DEPTH: u32 = 64;
const XOBJECT: Name = Name::literal(b"/XObject\0");
const SUBTYPE: Name = Name::literal(b"/Subtype\0");
const MATRIX: Name = Name::literal(b"/Matrix\0");
const FONT: Name = Name::literal(b"/Font\0");
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
pub(crate) struct PageResources<'a> {
    /// The resource dictionary itself.
    dictionary: ObjectHandle<'a>,
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

impl<'a> PageResources<'a> {
    /// Resolve a page's `/Resources`, climbing `/Parent` when the page declares none.
    ///
    /// # `/Resources` is inheritable, like `/Rotate`
    ///
    /// A page with no `/Resources` of its own uses the nearest ancestor's. `rotate.rs` makes
    /// the same climb for `/Rotate` and `sharing.rs` for this very key; a resolver that stopped
    /// at the page dictionary would report "this page names no fonts" for every document that
    /// puts them on a `/Pages` node — which is how a word processor writes a shared letterhead.
    ///
    /// Note what the committed corpus does **not** exercise: `mixed-rotation-4page.pdf` and
    /// `inherited-rotation-6page.pdf` carry an **empty** `/Resources`, not an absent one, so
    /// they refuse with `font-missing` either way. Absent and empty are different, and only one
    /// of them inherits.
    ///
    /// # A `/Parent` chain that does not terminate yields empty resources, not an error
    ///
    /// This section claimed `Error::Malformed` for that case and the function does not raise
    /// one: the climb falls out of its bounded loop and returns the empty dictionary. The
    /// direction is safe today — the resolver then refuses `font-missing` on the first glyph —
    /// but `redact_frame::inherited_numbers` *does* refuse in the same situation, so the two
    /// differ and only one of them said so.
    ///
    /// Corrected rather than changed: making this refuse would change what a document with a
    /// long `/Parent` chain does, which is a behaviour question rather than a doc one.
    ///
    /// # Errors
    ///
    /// Whatever qpdf latched while reading the page or an ancestor.
    pub(crate) fn of(owner: &ObjectHandle<'a>) -> Result<Self> {
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
                return Ok(Self {
                    dictionary: inherited,
                    fonts: std::cell::RefCell::new(BTreeMap::new()),
                });
            }
            current = current.key(&PARENT);
        }
        Ok(Self {
            dictionary: direct,
            fonts: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    /// The resource dictionary, for a caller that walks it itself.
    pub(crate) const fn dictionary(&self) -> &ObjectHandle<'a> {
        &self.dictionary
    }

    fn category(&self, category: &Name) -> ObjectHandle<'a> {
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
    /// Its own form's `/Resources` first, then the page's — the inheritance `Resources::within`
    /// applies, so this resolves a name the way the walk that read it did. Anything that does
    /// not is a bypass by construction; [`ScopedFont`] carries the five times that mattered.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the name resolves in neither. The walk already placed a glyph
    /// through it, so this is burrow disagreeing with itself rather than the document being
    /// wrong — and a silent `None` would be the narrowing every leak here has been.
    pub(crate) fn font_in_scope(&self, font: &ScopedFont) -> Result<ObjectHandle<'a>> {
        const RESOURCES: Name = Name::literal(b"/Resources\0");
        const XOBJECT: Name = Name::literal(b"/XObject\0");
        let key = Name::from_stripped(font.name())?;
        if let Some(id) = font.drawn_in() {
            // BY IDENTITY, not by name: the form is known by the object it is, and its entry in
            // the page's `/XObject` is where its own `/Resources` hang.
            let xobjects = self.category(&XOBJECT);
            for entry_key in crate::pdfsyntax::dict::top_level_keys(&xobjects.unparse())? {
                let entry = xobjects.key(&Name::from_stripped(&entry_key)?);
                if entry.type_code() != object_type::STREAM {
                    continue;
                }
                let (number, generation) = entry.object()?;
                let packed = (u64::from(number.unsigned_abs()) << 16)
                    | u64::from(generation.unsigned_abs() & 0xffff);
                if packed != id {
                    continue;
                }
                let own = entry.stream_dict().key(&RESOURCES);
                if own.type_code() == object_type::DICTIONARY {
                    let found = own.key(&FONT).key(&key);
                    if found.type_code() == object_type::DICTIONARY {
                        return Ok(found);
                    }
                }
                break;
            }
        }
        let found = self.category(&FONT).key(&key);
        if found.type_code() == object_type::DICTIONARY {
            return Ok(found);
        }
        Err(Error::Internal(
            "pdf resources: a font the walk drew a glyph with resolves in neither the form's \
             own resources nor the page's"
                .to_owned(),
        ))
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

impl<'a> Resources for PageResources<'a> {
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
        if entry.type_code() != object_type::STREAM {
            // An image, or a name the resources do not have. Neither draws glyphs.
            return Ok(None);
        }
        let dictionary = entry.stream_dict();
        if !names(&dictionary.key(&SUBTYPE), &SUBTYPE_FORM) {
            return Ok(None);
        }
        let Some(content) = entry.stream_data()? else {
            return Err(Error::Malformed(
                "pdf resources [form-unreadable]: a Form XObject whose data burrow could not \
                 decode, so what it draws is unknown"
                    .to_owned(),
            ));
        };
        let matrix = numbers_of(&dictionary.key(&MATRIX));
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
fn base_encoding(font: &ObjectHandle<'_>) -> crate::pdfsyntax::standard14::BaseEncoding {
    use crate::pdfsyntax::standard14::BaseEncoding;
    const BASE_ENCODING: Name = Name::literal(b"/BaseEncoding\0");
    const WIN_ANSI: Name = Name::literal(b"/WinAnsiEncoding\0");

    let encoding = font.key(&ENCODING);
    match encoding.type_code() {
        object_type::NAME => {
            if names(&encoding, &WIN_ANSI) {
                BaseEncoding::WinAnsi
            } else {
                BaseEncoding::Standard
            }
        }
        object_type::DICTIONARY => {
            if names(&encoding.key(&BASE_ENCODING), &WIN_ANSI) {
                BaseEncoding::WinAnsi
            } else {
                BaseEncoding::Standard
            }
        }
        _ => BaseEncoding::Standard,
    }
}

/// Read one font dictionary.
fn read_font(font: &ObjectHandle<'_>) -> Result<FontFacts> {
    let subtype = font.key(&SUBTYPE);
    let mut facts = FontFacts {
        first_char: integer_or(&font.key(&FIRST_CHAR), 0),
        widths: numbers_of(&font.key(&WIDTHS)),
        missing_width: None,
        // 0.001 for every font but Type 3, which declares its own.
        font_matrix: Matrix::scale(0.001, 0.001),
        font_bbox: rect_of(&font.key(&FONT_BBOX)),
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
        let missing = numbers_of(&descriptor.key(&MISSING_WIDTH));
        facts.missing_width = missing.first().copied();
    }

    if names(&subtype, &SUBTYPE_TYPE3) {
        match numbers_of(&font.key(&FONT_MATRIX)).as_slice() {
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

    if names(&subtype, &SUBTYPE_TYPE0) {
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
        facts.base_encoding = base_encoding(font);
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
fn read_composite(font: &ObjectHandle<'_>, facts: &mut FontFacts) -> Result<()> {
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
            let wmode = numbers_of(&dictionary.key(&WMODE))
                .first()
                .and_then(|value| {
                    // `as` TRUNCATES SILENTLY and this crate denies it. A `/WMode` that is not
                    // a whole small number is not a writing mode; refusing to read one is the
                    // conservative direction, and `writing_mode_of` refuses it by name.
                    whole(*value)
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
    facts.default_width = numbers_of(&descendant.key(&DW))
        .first()
        .copied()
        .or(Some(1000.0));
    facts.cid_widths = parse_w(&descendant.key(&W));
    Ok(())
}

/// `/W`, in either of its two shapes: `c [w …]` and `cFirst cLast w`.
fn parse_w(array: &ObjectHandle<'_>) -> BTreeMap<u32, f64> {
    let mut widths = BTreeMap::new();
    if array.type_code() != object_type::ARRAY {
        return widths;
    }
    let length = array.array_len();
    let mut at = 0;
    while at < length {
        let first = numbers_of(&array.array_item(at)).first().copied();
        let Some(start) = first else { break };
        let next = array.array_item(at + 1);
        if next.type_code() == object_type::ARRAY {
            for (offset, width) in numbers_of(&next).into_iter().enumerate() {
                let Some(base) = whole(start) else { break };
                let Ok(offset) = i64::try_from(offset) else {
                    break;
                };
                if let Ok(code) = u32::try_from(base.saturating_add(offset)) {
                    widths.insert(code, width);
                }
            }
            at += 2;
        } else {
            let Some(end) = numbers_of(&next).first().copied() else {
                break;
            };
            let Some(width) = numbers_of(&array.array_item(at + 2)).first().copied() else {
                break;
            };
            let (Some(start), Some(end)) = (whole(start), whole(end)) else {
                break;
            };
            // A RANGE THE FILE CHOOSES. Bounded so a `/W` of `[0 4294967295 500]` does not
            // materialise four billion entries in a map.
            for code in start..=end.min(start + 65_535) {
                if let Ok(code) = u32::try_from(code) {
                    widths.insert(code, width);
                }
            }
            at += 3;
        }
    }
    widths
}

/// Whether a name-valued handle names `want`.
///
/// # The slash is on the read side too, and the type now says so
///
/// `ObjectHandle::name` returns a [`Name`], which carries its leading `/` by construction, and
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
fn names(handle: &ObjectHandle<'_>, want: &Name) -> bool {
    handle.name().is_ok_and(|found| found == *want)
}

/// A `f64` that is exactly a whole number, as an `i64`.
///
/// `as` truncates silently and this crate denies it: a `/FirstChar` of `65.7` is not a
/// character code, and rounding it to 65 would place every glyph in the font by one code.
pub(super) fn whole(value: f64) -> Option<i64> {
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

/// Every number in a handle, whether it is one number or an array of them.
///
/// Through `unparse` and the content-stream lexer, because there is no trapped accessor for a
/// real: `qpdf_oh_get_int_value` truncates and says nothing about it.
fn numbers_of(handle: &ObjectHandle<'_>) -> Vec<f64> {
    let code = handle.type_code();
    if code == object_type::NULL {
        return Vec::new();
    }
    let text = handle.unparse();
    crate::pdfsyntax::ops::numbers_in(&text)
}

fn integer_or(handle: &ObjectHandle<'_>, fallback: i64) -> i64 {
    numbers_of(handle)
        .first()
        .and_then(|value| whole(*value))
        .unwrap_or(fallback)
}

fn rect_of(handle: &ObjectHandle<'_>) -> Option<Rect> {
    match numbers_of(handle).as_slice() {
        [left, bottom, right, top] => Some(Rect {
            left: left.min(*right),
            bottom: bottom.min(*top),
            right: left.max(*right),
            top: bottom.max(*top),
        }),
        _ => None,
    }
}
