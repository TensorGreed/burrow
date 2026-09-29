//! A page's display frame, read out of its dictionary.
//!
//! [`pdfsyntax::region::PageFrame`](crate::pdfsyntax::region::PageFrame) is what a region is
//! measured against: which box the viewer shows, which way `/Rotate` turned it, and what
//! `/UserUnit` makes a point mean. The region type carries the meaning; this reads the numbers.
//!
//! `/CropBox` falls back to `/MediaBox`, and `/MediaBox` is **inheritable** like `/Resources`
//! and `/Rotate` — a page that declares neither takes its ancestor's, and reading "no box" for
//! such a page would convert every region against a zero-sized frame.

use burrow_types::{Error, Result};

use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::geometry::Rect;
use crate::pdfsyntax::region::PageFrame;
use crate::redact::graph::PdfObject;

const CROP_BOX: Name = Name::literal(b"/CropBox\0");
const MEDIA_BOX: Name = Name::literal(b"/MediaBox\0");
const ROTATE: Name = Name::literal(b"/Rotate\0");
const USER_UNIT: Name = Name::literal(b"/UserUnit\0");
const PARENT: Name = Name::literal(b"/Parent\0");

/// How far up the page tree an inheritable key is looked for, matching `rotate`'s ceiling.
const MAX_PAGE_TREE_DEPTH: u32 = 64;

/// Read `page`'s display frame, the way the renderer a person sees it through reads it.
///
/// # Read as PDFium reads, or refused (#224)
///
/// Every value is read by its type, one reader per kind, and a value present in a shape the
/// reader does not take is **refused rather than climbed past or picked apart**. This read
/// boxes and `/Rotate` by pulling the numbers out of their text, so `/Rotate [90]` rotated,
/// `/CropBox [[0 0 300 200]]` cropped and a dictionary of four numbers cropped -- while PDFium,
/// which reads by type, ignored all three; each measured `Ok` with the secret left where PDFium
/// drew it. And the display box is the crop clipped to the media box, as PDFium clips it: a
/// `/CropBox` larger than the `/MediaBox` was measured the same way.
///
/// # Errors
///
/// - [`Error::Malformed`] `[no-display-box]` when no `/MediaBox` can be found, which leaves
///   nothing to measure a region against -- refused rather than defaulted to a letter page,
///   because a default frame converts every region to the wrong place silently.
/// - [`Error::Unsupported`] `[page-attribute-inherited]` for a `/CropBox` or `/Rotate` from the
///   page tree; `[page-frame-unreadable]` for a box, `/Rotate` or `/UserUnit` in a shape the
///   renderer does not read as burrow would, or a crop box that shares no area with the media
///   box; `[user-unit-not-one]` for a `/UserUnit` other than 1.
/// - [`Error::Malformed`] `[rotate-not-whole]` and `[page-tree-depth]`, as before.
pub(crate) fn of<O: PdfObject>(page: &O) -> Result<PageFrame> {
    let (crop, crop_from) = inherited(page, &CROP_BOX, "/CropBox", read_box)?;
    if crop_from == Found::Ancestor {
        return Err(inherited_refusal("/CropBox"));
    }
    let (media, _) = inherited(page, &MEDIA_BOX, "/MediaBox", read_box)?;
    let Some(media) = media else {
        return Err(Error::Malformed(
            "pdf redaction [no-display-box]: a page with no /MediaBox anywhere up its tree, so \
             there is nothing to measure a region against"
                .to_owned(),
        ));
    };
    // CLIPPED, AS THE RENDERER CLIPS IT. PDFium shows the crop box's overlap with the media
    // box; this used the crop box as written, and a crop larger than the media box put the
    // region against a page taller than the one shown (#224's review, measured).
    let display_box = match crop {
        None => media,
        Some(crop) => {
            let clipped = Rect {
                left: crop.left.max(media.left),
                bottom: crop.bottom.max(media.bottom),
                right: crop.right.min(media.right),
                top: crop.top.min(media.top),
            };
            if clipped.right <= clipped.left || clipped.top <= clipped.bottom {
                return Err(unreadable(
                    "a /CropBox that shares no area with the /MediaBox",
                ));
            }
            clipped
        }
    };

    let (rotate, rotate_from) = inherited(page, &ROTATE, "/Rotate", read_number)?;
    if rotate_from == Found::Ancestor {
        return Err(inherited_refusal("/Rotate"));
    }
    let rotate = rotate.unwrap_or(0.0);
    // A BOUNDED ANGLE. PDFium parses an integer that does not fit its own as 0, and a `/Rotate`
    // of 4294967490 is 90 to a reader that keeps the digits (measured, #224's review). A page
    // turned by more than ten full turns says nothing a person would write, so it is refused.
    if rotate.abs() > MAX_ROTATE {
        return Err(unreadable("a /Rotate larger than any reader agrees on"));
    }
    // NORMALISED, INCLUDING NEGATIVES. `/Rotate -90` is legal and means 270; `%` on a negative
    // yields a negative in Rust, which `Region::to_content_space` would refuse as not a right
    // angle — a refusal for a document every viewer displays.
    //
    // `as` is denied in this crate and it is denied for a reason that bites here: a `/Rotate`
    // of `1e300` truncates to a value with no relation to it, and the region would then be
    // unwound by an angle the file never named. `whole` refuses anything that is not an exact
    // integer, and a `/Rotate` that is not is not a right angle either.
    let Some(exact) = super::resources::whole(rotate) else {
        return Err(Error::Malformed(
            "pdf redaction [rotate-not-whole]: a page whose /Rotate is not a whole number of \
             degrees, so which way up it sits is not stated"
                .to_owned(),
        ));
    };
    let normalised = ((exact % 360) + 360) % 360;
    let rotate = u16::try_from(normalised).unwrap_or(0);

    // /UserUnit OTHER THAN 1 IS REFUSED until what it means across readers is settled: PDFium's
    // page size and its render both ignore it, measured, and the region conversion divides by
    // it, so the two disagree by that factor wherever the caller's units are the renderer's.
    let user_unit = read_number(page, &USER_UNIT, "/UserUnit")?.unwrap_or(1.0);
    if (user_unit - 1.0).abs() > f64::EPSILON {
        return Err(Error::Unsupported(
            "pdf redaction [user-unit-not-one]: a page whose /UserUnit is not 1, which the \
             renderer a person sees it through ignores, so the region and the page would be \
             measured in different units"
                .to_owned(),
        ));
    }

    Ok(PageFrame {
        display_box,
        rotate,
        user_unit,
    })
}

/// Where an inheritable key's value was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// On the page itself.
    Page,
    /// On an ancestor, because the page's own value is absent -- or null, or not a value this
    /// reads, which the engine cannot tell apart from absent (#224).
    Ancestor,
    /// Nowhere up the tree.
    Nowhere,
}

/// The refusal for a page that takes `key` from the page tree (#224).
///
/// # Why an inherited value is refused, and not only a null one
///
/// PDF says a page without `key` takes its ancestor's. A page that sets `key` to `null` does not:
/// PDFium, the renderer a person sees the page through, stops at the null (measured: `/Rotate`,
/// `/CropBox`, `/MediaBox` and `/Resources` alike), while qpdf reads a null exactly as it reads an
/// absent key and climbs. burrow reads the page through qpdf, so it cannot see the null, and the
/// two readings of one page differ: the region is measured against a frame the viewer does not
/// show, and a review got that to `Ok` with the secret still on the page. Refusing only the null
/// is not available; refusing every inherited value is. For `/CropBox` and `/Rotate` it cost
/// nothing on the 200 documents measured, which is one person's collection rather than the
/// world's, so the message says why a document was refused rather than only that it was.
fn inherited_refusal(key: &str) -> Error {
    Error::Unsupported(format!(
        "pdf redaction [page-attribute-inherited]: a page that takes its {key} from the page tree \
         instead of declaring it. A page that overrides it with null is shown without it, but \
         burrow cannot tell a null from an absent entry, so it cannot know which page the region \
         was drawn on"
    ))
}

/// PDFium's `/MediaBox` when a page's own is null: US Letter at the origin (measured, #224).
const RENDERER_NULL_MEDIA_BOX: Rect = Rect {
    left: 0.0,
    bottom: 0.0,
    right: 612.0,
    top: 792.0,
};

/// Refuse a page whose `/MediaBox` is inherited unless a second reading vouches for it (#224).
///
/// `renderer_size` is the size the renderer gives the displayed page -- its box, rotated -- read
/// without loading the page's content. Inherited `/CropBox` and `/Rotate` are refused by [`of`],
/// and the page's own are read alike by both, so the renderer's `/MediaBox` is one of two:
/// the inherited box, or [`RENDERER_NULL_MEDIA_BOX`] if the page's own was null. Where the two
/// differ in size, the renderer's size says which it used. Where they do not -- an inherited
/// Letter-sized box away from the origin, with no `/CropBox` of the page's own to clip both --
/// a size cannot tell them apart, and the page is refused. With the page's own `/CropBox`, both
/// readers show at most that box and the renderer shows exactly it only if its size is that
/// box's, so a size decides.
///
/// # One rule and one message, for three reasons
///
/// `media-box-unverified` whether there is no second reading (the web engine, until the renderer
/// reaches redaction), a size cannot decide, or the renderer's size is not burrow's. All three
/// mean one thing -- nothing confirmed the inherited box is the one the viewer shows -- and the
/// refusal says only that, word for word the same on both engines: the native-backed web
/// differential compares whole outcomes, and a null `/MediaBox` is refused natively for the third
/// reason and on the web for the first. Which reason applied is in the code path, not the text.
///
/// # Errors
///
/// [`Error::Unsupported`] naming `media-box-unverified`, for each of the three, and for a
/// renderer that fails to answer; a limit the renderer hits passes through as that limit; and
/// whatever [`of`] raises for the page.
pub(crate) fn check_inherited_media_box<O: PdfObject>(
    page: &O,
    renderer_size: impl FnOnce() -> Result<Option<(f64, f64)>>,
) -> Result<()> {
    // `of` FIRST: it refuses every shape the typed readers do not take, so a box found on an
    // ancestor below is always a box. The first version matched `(Some(box), Ancestor)` and let
    // an ancestor value that did not read as a box skip the check entirely (#224's review).
    let frame = of(page)?;
    let (inherited_box, Found::Ancestor) = inherited(page, &MEDIA_BOX, "/MediaBox", read_box)?
    else {
        return Ok(());
    };
    let Some(inherited_box) = inherited_box else {
        return Err(unverified());
    };
    let own_crop = read_box(page, &CROP_BOX, "/CropBox")?;
    // A RENDERER THAT CANNOT ANSWER IS NOT AN ANSWER: its failure (a page it numbers otherwise,
    // a document it will not open) is this rule's refusal, so both engines say the same thing.
    // A limit stays a limit: running out of budget is the operation's outcome, not a verdict.
    let answer = match renderer_size() {
        Ok(answer) => answer,
        Err(limit @ Error::LimitExceeded { .. }) => return Err(limit),
        Err(_) => return Err(unverified()),
    };
    let Some((width, height)) = answer else {
        // NO SECOND READING: the web engine, until the renderer reaches redaction (#206).
        return Err(unverified());
    };
    let same_size = |a: &Rect, b: &Rect| {
        ((a.right - a.left) - (b.right - b.left)).abs() < SIZE_TOLERANCE
            && ((a.top - a.bottom) - (b.top - b.bottom)).abs() < SIZE_TOLERANCE
    };
    if own_crop.is_none()
        && inherited_box != RENDERER_NULL_MEDIA_BOX
        && same_size(&inherited_box, &RENDERER_NULL_MEDIA_BOX)
    {
        // A SIZE CANNOT DECIDE: the inherited box is Letter-sized away from the origin.
        return Err(unverified());
    }
    let shown = &frame.display_box;
    let (ours_w, ours_h) = if frame.rotate == 90 || frame.rotate == 270 {
        (shown.top - shown.bottom, shown.right - shown.left)
    } else {
        (shown.right - shown.left, shown.top - shown.bottom)
    };
    if (ours_w - width).abs() >= SIZE_TOLERANCE || (ours_h - height).abs() >= SIZE_TOLERANCE {
        // THE RENDERER DISAGREES: it shows the page at another size than burrow reads.
        return Err(unverified());
    }
    Ok(())
}

/// How far two sizes may differ and be one size: the renderer reports `f32`.
const SIZE_TOLERANCE: f64 = 0.01;

fn unverified() -> Error {
    Error::Unsupported(
        "pdf redaction [media-box-unverified]: a page that takes its /MediaBox from the page tree, \
         and nothing confirmed it is the box the viewer shows. A page that overrides it with null \
         is shown at US Letter, and burrow cannot tell a null from an absent entry"
            .to_owned(),
    )
}

/// The largest `/Rotate` read: ten full turns either way. See `of`.
const MAX_ROTATE: f64 = 3600.0;

/// A value in a shape the renderer does not read as this would (#224).
fn unreadable(what: &str) -> Error {
    Error::Unsupported(format!(
        "pdf redaction [page-frame-unreadable]: {what}. The renderer a person sees the page \
         through reads such a value differently, so the region would be measured against a \
         different page than the one shown"
    ))
}

/// An inheritable key, read by `read` on the page and then up the tree, and where it was found.
///
/// Absent -- or null, which qpdf reads the same -- climbs. A value `read` does not take is
/// refused where it is, never climbed past: PDFium stops at any present value (#224).
fn inherited<O: PdfObject, T>(
    page: &O,
    key: &Name,
    label: &str,
    read: fn(&O, &Name, &str) -> Result<Option<T>>,
) -> Result<(Option<T>, Found)> {
    if let Some(value) = read(page, key, label)? {
        return Ok((Some(value), Found::Page));
    }
    let mut current = page.key(&PARENT);
    for _ in 0..MAX_PAGE_TREE_DEPTH {
        if current.type_code() != object_type::DICTIONARY {
            return Ok((None, Found::Nowhere));
        }
        current.drained()?;
        if let Some(value) = read(&current, key, label)? {
            return Ok((Some(value), Found::Ancestor));
        }
        current = current.key(&PARENT);
    }
    Err(Error::Malformed(
        "pdf redaction [page-tree-depth]: a /Parent chain deeper than burrow will climb".to_owned(),
    ))
}

/// `node`'s `key` as a box: absent, or an array of exactly four numbers, one per item.
fn read_box<O: PdfObject>(node: &O, key: &Name, label: &str) -> Result<Option<Rect>> {
    let value = node.key(key);
    let code = value.type_code();
    if code == object_type::NULL {
        return Ok(None);
    }
    if code != object_type::ARRAY || value.array_len() != 4 {
        return Err(unreadable(&format!("a {label} that is not four numbers")));
    }
    let mut corners = [0.0_f64; 4];
    for (at, corner) in (0..4).zip(corners.iter_mut()) {
        *corner = one_number(&value.array_item(at))
            .ok_or_else(|| unreadable(&format!("a {label} that is not four numbers")))?;
    }
    let [left, bottom, right, top] = corners;
    Ok(Some(Rect {
        left: left.min(right),
        bottom: bottom.min(top),
        right: left.max(right),
        top: bottom.max(top),
    }))
}

/// `node`'s `key` as a number: absent, or one integer or real.
fn read_number<O: PdfObject>(node: &O, key: &Name, label: &str) -> Result<Option<f64>> {
    let value = node.key(key);
    if value.type_code() == object_type::NULL {
        return Ok(None);
    }
    one_number(&value)
        .map(Some)
        .ok_or_else(|| unreadable(&format!("a {label} that is not a number")))
}

/// The one finite number an integer or real handle holds, through its text: there is no trapped
/// accessor for a real, and `unparse` resolves an indirect reference to the value it names.
fn one_number<O: PdfObject>(handle: &O) -> Option<f64> {
    let code = handle.type_code();
    if code != object_type::INTEGER && code != object_type::REAL {
        return None;
    }
    match crate::pdfsyntax::ops::numbers_in(&handle.unparse())[..] {
        [number] if number.is_finite() => Some(number),
        _ => None,
    }
}
