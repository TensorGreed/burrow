//! The pixel ceiling, and the one conversion every render does.
//!
//! Ungated and platform-free, like [`crate::estimate`] and [`crate::codes`], and for the same
//! reason: the native path and the web path must apply **the same** ceiling with the same
//! arithmetic and produce the same bytes, and two copies of that cannot be relied on to stay
//! identical. Only the call that fills the engine's bitmap differs between them.
//!
//! [ADR 0027](../../../docs/adr/0027-what-a-render-promises-and-what-it-refuses.md) is the
//! record; the two things worth repeating here are the ones a reader of this file needs:
//!
//! - **`max_pixels` is checked before the bitmap is allocated, never after.** The whole value
//!   of the check is that the allocation does not happen.
//! - **`Limits::DEFAULT.max_pixels` is a caller ceiling and is far too loose to be a device
//!   ceiling**: the PDF maximum page at 1:1 is 207 Mpx and passes it. What bounds a browser
//!   tab is the smaller number the web app asks for.

use burrow_types::{Error, Limits, Result, Stage};

/// Bytes per pixel in a [`Raster`](crate::Raster). RGBA, eight bits a channel.
pub(crate) const BYTES_PER_PIXEL: u64 = 4;

/// The same number as a `usize`, written once rather than cast at four call sites.
///
/// A `as usize` cast is denied across this workspace precisely because it is silent on a
/// 32-bit target; a second constant costs a line and cannot truncate.
const BYTES_PER_PIXEL_USIZE: usize = 4;

/// Check a requested raster against `max_pixels`, and return the pixel count.
///
/// Called immediately before the engine is asked to allocate, on both platforms.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] — either dimension is zero. A zero-pixel raster is not a small
///   raster; it is a request that cannot be satisfied, and refusing it here means no engine
///   has to have an opinion about it.
/// - [`Error::LimitExceeded`] — `width * height` exceeds `limits.max_pixels`, at
///   [`Stage::Pixels`], or the resulting buffer would not fit in a `usize` on this target.
pub(crate) fn check_pixels(width: u32, height: u32, limits: &Limits) -> Result<u64> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidArgument(
            "a render needs a non-zero width and height".to_owned(),
        ));
    }

    // `u64` throughout: two `u32`s multiply to at most 2^64 - 2^33 + 1, so this cannot wrap,
    // and doing it in `u32` is exactly how a ceiling check ends up passing a request that
    // overflowed into something small.
    let pixels = u64::from(width) * u64::from(height);
    Limits::check(Stage::Pixels, "max_pixels", pixels, limits.max_pixels)?;

    // The buffer the caller will be handed. On wasm32 a `usize` is 32 bits, so a request that
    // is under `max_pixels` on a 64-bit host can still be unrepresentable here -- and finding
    // that out at the allocation rather than at the ceiling would report it as an engine
    // failure rather than as the refusal it is.
    let bytes = pixels
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or_else(|| Error::Internal("raster byte count does not fit in u64".to_owned()))?;
    if usize::try_from(bytes).is_err() {
        // What this target could hold, expressed in PIXELS, so the number the caller is told
        // about is in the same unit as the limit they set. `usize::MAX` is at least 2^32 - 1
        // on every target this builds for, so the widening cannot fail.
        let addressable = u64::try_from(usize::MAX)
            .map_err(|_| Error::Internal("usize does not fit in u64".to_owned()))?;
        return Err(Error::LimitExceeded {
            limit: "max_pixels",
            stage: Stage::Pixels,
            requested: pixels,
            #[allow(
                clippy::integer_division,
                reason = "a byte count converted to a pixel count; the remainder is not a pixel"
            )]
            allowed: addressable / BYTES_PER_PIXEL,
        });
    }

    Ok(pixels)
}

/// Copy PDFium's BGRA bitmap into a packed RGBA buffer.
///
/// `src` is `stride * height` bytes as the engine laid them out. **`stride` is read from the
/// engine and may exceed `width * 4`** — PDFium pads rows — so this walks row by row. A
/// version that assumed `width * 4` would return the wrong pixels on exactly the pages where
/// padding happens, and be correct everywhere else, which is the worst shape a bug can have.
///
/// # Errors
///
/// [`Error::Internal`] if `stride` is smaller than a row, or `src` is shorter than
/// `stride * height`. Both mean the engine contradicted itself; neither is the document's
/// fault and neither is reported as if it were.
pub(crate) fn bgra_to_rgba(src: &[u8], stride: usize, width: u32, height: u32) -> Result<Vec<u8>> {
    let width = usize::try_from(width)
        .map_err(|_| Error::Internal("raster width does not fit in usize".to_owned()))?;
    let height = usize::try_from(height)
        .map_err(|_| Error::Internal("raster height does not fit in usize".to_owned()))?;
    let row_bytes = width
        .checked_mul(BYTES_PER_PIXEL_USIZE)
        .ok_or_else(|| Error::Internal("raster row does not fit in usize".to_owned()))?;
    if stride < row_bytes {
        return Err(Error::Internal(
            "the engine reported a stride narrower than one row".to_owned(),
        ));
    }
    let needed = stride
        .checked_mul(height)
        .ok_or_else(|| Error::Internal("raster does not fit in usize".to_owned()))?;
    if src.len() < needed {
        return Err(Error::Internal(
            "the engine returned fewer bytes than its own stride implies".to_owned(),
        ));
    }

    let mut out = Vec::with_capacity(row_bytes.saturating_mul(height));
    for row in 0..height {
        let start = row * stride;
        let line = src
            .get(start..start + row_bytes)
            .ok_or_else(|| Error::Internal("raster row is out of range".to_owned()))?;
        // `as_chunks` rather than `chunks_exact`: it yields `&[u8; 4]`, so the length is in
        // the type and the compiler discharges it. The remainder is empty by construction --
        // `row_bytes` is a multiple of four -- and is ignored for that reason.
        let (pixels, _remainder) = line.as_chunks::<BYTES_PER_PIXEL_USIZE>();
        for pixel in pixels {
            // PDFium's `FPDFBitmap_BGRA` is little-endian 0xAARRGGBB, which in memory is
            // B, G, R, A. A slice pattern rather than four indices -- `chunks_exact` already
            // guarantees the length, and this is the spelling that says so to the compiler
            // instead of to a reader. A `reverse()` would be wrong: it would also move the
            // alpha channel, which is already last in both orders.
            let [b, g, r, a] = *pixel;
            out.extend_from_slice(&[r, g, b, a]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits_with_pixels(max: u64) -> Limits {
        Limits::with(|l| l.max_pixels = max)
    }

    #[test]
    fn a_request_at_the_ceiling_is_accepted() {
        let limits = limits_with_pixels(100);
        assert_eq!(check_pixels(10, 10, &limits).unwrap(), 100);
    }

    #[test]
    fn one_pixel_past_the_ceiling_is_refused_at_the_pixels_stage() {
        let limits = limits_with_pixels(99);
        let error = check_pixels(10, 10, &limits).unwrap_err();
        match error {
            Error::LimitExceeded {
                limit,
                stage,
                requested,
                allowed,
            } => {
                assert_eq!(limit, "max_pixels");
                assert_eq!(stage, Stage::Pixels);
                assert_eq!(requested, 100);
                assert_eq!(allowed, 99);
            }
            other => panic!("expected LimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn a_zero_dimension_is_an_invalid_argument_not_a_small_raster() {
        let limits = limits_with_pixels(u64::MAX);
        assert!(matches!(
            check_pixels(0, 10, &limits),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(
            check_pixels(10, 0, &limits),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn the_product_is_computed_in_u64_so_it_cannot_wrap_into_something_small() {
        // 65536 * 65536 is 2^32, which is 0 in u32 arithmetic and would sail under any
        // ceiling. The default is 256 Mpx, so this must be refused.
        let error = check_pixels(65_536, 65_536, &Limits::DEFAULT).unwrap_err();
        assert!(matches!(
            error,
            Error::LimitExceeded {
                requested: 4_294_967_296,
                ..
            }
        ));
    }

    #[test]
    fn the_pdf_maximum_page_at_one_to_one_passes_the_default_ceiling() {
        // ADR 0027's decisive finding, kept as a test so it cannot quietly stop being true:
        // 14400 x 14400 points is 207 Mpx and 791 MiB, and `Limits::DEFAULT` accepts it. The
        // default is a CALLER ceiling. What bounds a browser tab is the number the web app
        // asks for, and this test is what stops somebody reading the default as that bound.
        let pixels = check_pixels(14_400, 14_400, &Limits::DEFAULT).unwrap();
        assert_eq!(pixels, 207_360_000);
        assert!(pixels < Limits::DEFAULT.max_pixels);
        // 791 MiB at four bytes a pixel, and accepted.
        assert_eq!(pixels * BYTES_PER_PIXEL, 829_440_000);
    }

    #[test]
    fn the_ceiling_adr_0027_chose_refuses_a_full_page_at_three_hundred_dpi() {
        // The other half of the same point: 4 Mpx is what the WEB asks for, and it admits
        // A4 at 200 dpi (3.87 Mpx) while refusing A4 at 300 dpi (8.70 Mpx).
        let web = limits_with_pixels(4 * 1024 * 1024);
        assert!(check_pixels(1653, 2339, &web).is_ok());
        assert!(matches!(
            check_pixels(2479, 3508, &web),
            Err(Error::LimitExceeded {
                stage: Stage::Pixels,
                ..
            })
        ));
    }

    #[test]
    fn bgra_becomes_rgba_and_padding_is_skipped() {
        // Two pixels wide, two rows, with four bytes of row padding: the case a reader that
        // assumed `width * 4` gets wrong and only on pages where PDFium pads.
        let stride = 12;
        let src = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 0xAA, 0xAA, 0xAA, 0xAA, // row 0 + padding
            9, 10, 11, 12, 13, 14, 15, 16, 0xBB, 0xBB, 0xBB, 0xBB, // row 1 + padding
        ];
        let out = bgra_to_rgba(&src, stride, 2, 2).unwrap();
        assert_eq!(
            out,
            vec![3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]
        );
    }

    #[test]
    fn a_stride_narrower_than_a_row_is_the_engine_contradicting_itself() {
        let src = vec![0_u8; 64];
        assert!(matches!(
            bgra_to_rgba(&src, 4, 2, 2),
            Err(Error::Internal(_))
        ));
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_read_past() {
        let src = vec![0_u8; 8];
        assert!(matches!(
            bgra_to_rgba(&src, 8, 2, 2),
            Err(Error::Internal(_))
        ));
    }
}
