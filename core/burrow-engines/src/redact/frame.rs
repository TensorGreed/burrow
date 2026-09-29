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

/// Read `page`'s display frame.
///
/// # Errors
///
/// [`Error::Malformed`] when no box can be found, which leaves nothing to measure a region
/// against — refused rather than defaulted to a letter page, because a default frame converts
/// every region to the wrong place silently.
pub(crate) fn of<O: PdfObject>(page: &O) -> Result<PageFrame> {
    let (crop, crop_from) = inherited_rect(page, &CROP_BOX)?;
    if crop_from == Found::Ancestor {
        return Err(inherited("/CropBox"));
    }
    let (media, _) = inherited_rect(page, &MEDIA_BOX)?;
    let display_box = crop.or(media).ok_or_else(|| {
        Error::Malformed(
            "pdf redaction [no-display-box]: a page with neither /CropBox nor /MediaBox, \
                 anywhere up its tree, so there is nothing to measure a region against"
                .to_owned(),
        )
    })?;

    let (rotate, rotate_from) = inherited_numbers(page, &ROTATE)?;
    if rotate_from == Found::Ancestor {
        return Err(inherited("/Rotate"));
    }
    let rotate = rotate.first().copied().unwrap_or(0.0);
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

    let user_unit = numbers(&page.key(&USER_UNIT))
        .first()
        .copied()
        .unwrap_or(1.0);

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
fn inherited(key: &str) -> Error {
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
/// [`Error::Unsupported`] naming `media-box-unverified`, for each of the three.
pub(crate) fn check_inherited_media_box<O: PdfObject>(
    page: &O,
    renderer_size: impl FnOnce() -> Result<Option<(f64, f64)>>,
) -> Result<()> {
    let (Some(inherited_box), Found::Ancestor) = inherited_rect(page, &MEDIA_BOX)? else {
        return Ok(());
    };
    let frame = of(page)?;
    let (own_crop, _) = inherited_rect(page, &CROP_BOX)?;
    let Some((width, height)) = renderer_size()? else {
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

/// An inheritable rectangle-valued key, and where it was found.
fn inherited_rect<O: PdfObject>(page: &O, key: &Name) -> Result<(Option<Rect>, Found)> {
    let (found, from) = inherited_numbers(page, key)?;
    let rect = match found.as_slice() {
        [left, bottom, right, top] => Some(Rect {
            left: left.min(*right),
            bottom: bottom.min(*top),
            right: left.max(*right),
            top: bottom.max(*top),
        }),
        _ => None,
    };
    Ok((rect, from))
}

/// An inheritable key's numbers, climbing `/Parent` when the page declares none, and where
/// they were found.
fn inherited_numbers<O: PdfObject>(page: &O, key: &Name) -> Result<(Vec<f64>, Found)> {
    let direct = numbers(&page.key(key));
    if !direct.is_empty() {
        return Ok((direct, Found::Page));
    }
    let mut current = page.key(&PARENT);
    for _ in 0..MAX_PAGE_TREE_DEPTH {
        if current.type_code() != object_type::DICTIONARY {
            return Ok((Vec::new(), Found::Nowhere));
        }
        current.drained()?;
        let found = numbers(&current.key(key));
        if !found.is_empty() {
            return Ok((found, Found::Ancestor));
        }
        current = current.key(&PARENT);
    }
    Err(Error::Malformed(
        "pdf redaction [page-tree-depth]: a /Parent chain deeper than burrow will climb".to_owned(),
    ))
}

fn numbers<O: PdfObject>(handle: &O) -> Vec<f64> {
    if handle.type_code() == object_type::NULL {
        return Vec::new();
    }
    crate::pdfsyntax::ops::numbers_in(&handle.unparse())
}
