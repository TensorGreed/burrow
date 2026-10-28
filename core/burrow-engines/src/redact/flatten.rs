//! Redaction v1's page replacement: page *N* becomes one picture of itself (ADR 0031 §1 step 7).
//!
//! # What this is, and what it is not yet
//!
//! This is the **write half** of ADR 0031's pipeline, and its first slice. Given the painted
//! bitmap of page *N* -- produced, in the slices that follow, by PDFium's 150 dpi render with the
//! region filled black -- it replaces page *N* **in place** with a page that draws only that
//! bitmap, writes the document, and reads the result back before any byte leaves.
//!
//! It is **not** an operation anyone can reach. Nothing in `burrow-core`, `burrow-wasm` or
//! `burrow-ffi` names it, and the redaction tool is held (#136 S9). It runs none of ADR 0031's
//! §2 or §5 refusals: the survivor and structure walks, the form and annotation rules, page
//! identity across the two engines, and V4's element walk and V5 to V7 are later slices. Until
//! they land, an `Ok` from here says only what `check_output` reads back, and says nothing
//! about whether page *N*'s old content is still reachable from something the output keeps.
//!
//! # The steps, in order
//!
//! 1. **The frames, read as redaction reads them** (`redact::frames`): page *N*'s display frame
//!    *d*w × *d*h, and every other page's, for V1.
//! 2. **The picture is held to that frame** (§1 step 6's size and length half): *W* and *H* are
//!    re-derived here as ⌈*d*·150/72⌉ by [`pixels_for`], never taken from whoever drew the
//!    picture; `max_pixels`, and redaction's own [`MAX_FLATTEN_PIXELS`], are applied to their
//!    product exactly; and the buffer must be exactly *W*·*H*·3 bytes.
//! 3. **The open forces qpdf's whole-file resolution inside the trap** (§1, *Resolution at
//!    open*), by making one null indirect: the first object number qpdf hands out resolves every
//!    cross-reference entry. A repair there refuses, `[engine-repaired-input]`. After it,
//!    `qpdf_oh_new_stream` finds the table resolved. **This slice asserts that the trapped call
//!    ran, not qpdf's `fixed_dangling_refs` flag**; reading the flag is the next slice (security
//!    L-6), and until then the repair refusal is read again after the constructors and after
//!    the write, so a resolution that slipped past the open is still refused.
//! 4. **Page *N* is rewritten in place** (§3): its dictionary keeps its object identity, every
//!    key but `/Type` and `/Parent` is removed, and `/Rotate 0`, `/MediaBox` and `/CropBox`
//!    `[0 0 dw dh]`, `/Resources` naming one image and `/Contents` holding one line are written.
//!    The catalogue loses `/StructTreeRoot` and `/MarkInfo` (§2 rule 1's drop; rule 1's
//!    `[structure-tree-kept]` is a later slice). **No destination, bead or other page is
//!    touched** (decision 74).
//! 5. **Write, then read back** through a fresh open: V1's frames, V2 and V3 (`check_output`).
//!    `burrow_ops` adds V1's page count and rotation vector through `verify::output`.

use burrow_types::{Error, Result};

/// The render's resolution, in dots per inch (ADR 0031 §2 rule 6).
pub const DPI: u32 = 150;

/// Redaction's own ceiling on the picture, in pixels, on every platform (ADR 0031 §6).
///
/// The RGB bytes of a bigger picture exceed the 256 MiB qpdf will decode, so verification could
/// not read it back: 89,478,485 × 3 = 268,435,455, one byte under 2^28. Applied beside
/// `max_pixels`, and exactly. On the web `max_pixels` (4,194,304) is the smaller and decides.
pub const MAX_FLATTEN_PIXELS: u64 = 89_478_485;

/// The page bitmap that replaces page *N*: 8-bit RGB, row-major from the top-left, no padding.
///
/// In display orientation, covering the display frame (§1 step 2). **Its size is never trusted**:
/// the write re-derives *W* and *H* from qpdf's frame and refuses a picture of any other size.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Picture<'a> {
    /// Width in pixels, as the drawing side reports it.
    pub width: u32,
    /// Height in pixels, as the drawing side reports it.
    pub height: u32,
    /// The pixels, three bytes each.
    pub rgb: &'a [u8],
}

impl<'a> Picture<'a> {
    /// A picture of `width` × `height` pixels.
    #[must_use]
    pub const fn new(width: u32, height: u32, rgb: &'a [u8]) -> Self {
        Self { width, height, rgb }
    }
}

/// The pixels a frame side of `points` needs at [`DPI`]: ⌈`points`·150/72⌉.
///
/// **The one place this is computed**, so that the picture's size, the check of it and (in a
/// later slice) the render agree by construction rather than by two formulas that happen to
/// match. Found by search over whole numbers rather than by a cast: `side * 150 / 72` is at most
/// 30,000 for a frame burrow accepts, and the smallest `n` with `n >= scaled` is exact in `f64`.
///
/// # Errors
///
/// [`Error::Internal`] for a side that is not finite and positive, or whose pixels would not fit
/// in a `u32`: `redact::frames` refuses such a frame first, so reaching this is a bug.
pub fn pixels_for(points: f64) -> Result<u32> {
    let scaled = points * f64::from(DPI) / 72.0;
    if !(scaled.is_finite() && scaled > 0.0 && scaled <= f64::from(u32::MAX)) {
        return Err(Error::Internal(
            "pdf flatten: a frame side with no pixel size at 150 dpi".to_owned(),
        ));
    }
    // THE SMALLEST WHOLE `n` NOT BELOW `scaled`: a binary search, every comparison exact.
    let (mut low, mut high) = (1_u32, u32::MAX);
    while low < high {
        let middle = low + ((high - low) >> 1);
        if f64::from(middle) >= scaled {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(low)
}

// THE WRITE ITSELF, native only until the web's constructors are exported (a later slice of ADR
// 0031's build): `BuildsObjects` has no web implementation yet, so the policy has nothing to run
// on there.
#[cfg(all(feature = "native-engines", burrow_native_engines))]
mod write;
#[cfg(all(feature = "native-engines", burrow_native_engines))]
pub(crate) use write::flatten;

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::{MAX_FLATTEN_PIXELS, pixels_for};

    /// ADR 0031 rule 6's ceilings, as sides: each exactly representable in `f32` and `f64`, so
    /// PDFium's 32-bit reading and qpdf's 64-bit one give the same *W* and *H* (code N3).
    #[test]
    fn the_ceiling_sides_give_the_adr_s_pixel_counts_in_both_widths() {
        let cases: [(f64, f64, u64); 4] = [
            (983.0, 983.0, 4_194_304),
            (983.0, 983.5, 2048 * 2049),
            (2992.5, 6888.25, MAX_FLATTEN_PIXELS),
            (2992.5, 6888.75, 6235 * 14352),
        ];
        for (width, height, pixels) in cases {
            let (wide, high) = (pixels_for(width).unwrap(), pixels_for(height).unwrap());
            assert_eq!(
                u64::from(wide) * u64::from(high),
                pixels,
                "{width} x {height}"
            );
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the f32 reading is the point"
            )]
            let (narrow_width, narrow_height) = (f64::from(width as f32), f64::from(height as f32));
            assert_eq!(pixels_for(narrow_width).unwrap(), wide);
            assert_eq!(pixels_for(narrow_height).unwrap(), high);
        }
    }

    #[test]
    fn pixels_round_up_and_land_exactly_on_whole_sides() {
        assert_eq!(pixels_for(72.0).unwrap(), 150);
        assert_eq!(pixels_for(612.0).unwrap(), 1275);
        assert_eq!(pixels_for(0.001).unwrap(), 1);
        assert_eq!(pixels_for(72.0001).unwrap(), 151);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(pixels_for(bad).is_err(), "{bad}");
        }
    }

    proptest::proptest! {
        /// `pixels_for` is the ceiling: never below the scaled side, and less than one above it.
        #[test]
        fn pixels_are_the_ceiling_of_the_scaled_side(side in 0.01_f64..14_400.0) {
            let scaled = side * 150.0 / 72.0;
            let pixels = f64::from(pixels_for(side).unwrap());
            proptest::prop_assert!(pixels >= scaled && pixels - scaled < 1.0);
        }

    }
}
