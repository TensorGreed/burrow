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

use super::Document;
use super::handle::ObjectHandle;
use super::name::Name;
use crate::codes::qpdf::object_type;
use crate::pdfsyntax::geometry::Rect;
use crate::pdfsyntax::region::PageFrame;

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
pub(super) fn of(document: &Document, page: &ObjectHandle<'_>) -> Result<PageFrame> {
    let display_box = inherited_rect(document, page, &CROP_BOX)?
        .or(inherited_rect(document, page, &MEDIA_BOX)?)
        .ok_or_else(|| {
            Error::Malformed(
                "pdf redaction [no-display-box]: a page with neither /CropBox nor /MediaBox, \
                 anywhere up its tree, so there is nothing to measure a region against"
                    .to_owned(),
            )
        })?;

    let rotate = inherited_numbers(document, page, &ROTATE)?
        .first()
        .copied()
        .unwrap_or(0.0);
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

/// An inheritable rectangle-valued key.
fn inherited_rect(
    document: &Document,
    page: &ObjectHandle<'_>,
    key: &Name,
) -> Result<Option<Rect>> {
    let found = inherited_numbers(document, page, key)?;
    Ok(match found.as_slice() {
        [left, bottom, right, top] => Some(Rect {
            left: left.min(*right),
            bottom: bottom.min(*top),
            right: left.max(*right),
            top: bottom.max(*top),
        }),
        _ => None,
    })
}

/// An inheritable key's numbers, climbing `/Parent` when the page declares none.
fn inherited_numbers(document: &Document, page: &ObjectHandle<'_>, key: &Name) -> Result<Vec<f64>> {
    let direct = numbers(&page.key(key));
    if !direct.is_empty() {
        return Ok(direct);
    }
    let mut current = page.key(&PARENT);
    for _ in 0..MAX_PAGE_TREE_DEPTH {
        if current.type_code() != object_type::DICTIONARY {
            return Ok(Vec::new());
        }
        if let Some(error) = document.take_error() {
            return Err(error);
        }
        let found = numbers(&current.key(key));
        if !found.is_empty() {
            return Ok(found);
        }
        current = current.key(&PARENT);
    }
    Err(Error::Malformed(
        "pdf redaction [page-tree-depth]: a /Parent chain deeper than burrow will climb".to_owned(),
    ))
}

fn numbers(handle: &ObjectHandle<'_>) -> Vec<f64> {
    if handle.type_code() == object_type::NULL {
        return Vec::new();
    }
    crate::pdfsyntax::ops::numbers_in(&handle.unparse())
}
