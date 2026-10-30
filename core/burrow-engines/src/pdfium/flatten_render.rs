//! Redaction v1's render of page *N*, painted in PDFium's heap (ADR 0031 §1 steps 2 and 4, native).
//!
//! # What happens, in order, on the engine thread
//!
//! 1. **Rule 6's ceilings, before anything is allocated**: *W* x *H* = ⌈*d*w·150/72⌉ x
//!    ⌈*d*h·150/72⌉ from the display frame qpdf read (`redact::flatten::pixels_for`), checked
//!    against `max_pixels` and `MAX_FLATTEN_PIXELS` exactly, then the page load's memory guard
//!    (`estimate::before_page_load`).
//! 2. **The render, as displayed** (§1 step 2): an opaque-white BGRA bitmap of *W* x *H*, drawn
//!    by the same progressive render every PDFium picture in this crate uses, with start (0, 0),
//!    size *W* x *H* and `rotate = 0`, so PDFium applies the page's `/Rotate`. **Its flags are
//!    `FPDF_ANNOT` and nothing else** ([`Annotations::Drawn`]), and no form-fill handle exists:
//!    §2 rule 9, the selection picture's setting.
//! 3. **The paint** (§1 step 4): the region, converted to user space by burrow's region
//!    conversion, mapped corner by corner through `FPDF_PageToDevice` with the render's own start,
//!    size and rotation, widened by one pixel for that call's rounding and one for rule 8
//!    (`redact::flatten::paint`), and filled opaque black by `FPDFBitmap_FillRect` -- **in
//!    PDFium's heap, before any pixel is copied out**. The unpainted pixels never leave it.
//! 4. **One copy out**, to the canonical RGB form, three bytes a pixel, rows top to bottom; then
//!    the bitmap is destroyed and the page closed, and the resident set read.
//!
//! The picture is **not** trusted by what follows: `redact::flatten::cross_check` holds its size,
//! length and paint to qpdf's frame and burrow's own arithmetic, and the write's read-back holds
//! the image it decodes to the picture.
//!
//! # What this does not do (later slices)
//!
//! Rule 7's record and digest (S9c); page identity across the two engines, the page count, every
//! page's no-load size and page *N*'s rotation (S9b, §5); the web's render worker (S12). Natively,
//! PDFium's heap is the process heap and is not wiped (§7, native posture is M3/M4's).
//!
//! # Limits
//!
//! `max_pixels` is exact, before `FPDFBitmap_Create`. `max_duration_ms` is checkpointed between
//! the render's slices and between the steps; the page load is one uninterruptible call, as for
//! every render. `max_memory_bytes` is **detected, not bounded**: guarded before the load, read
//! between slices and after the render. What PDFium's rasteriser allocates for itself is bounded
//! by nothing (ADR 0027).

#![allow(
    dead_code,
    reason = "ADR 0031 S9a: no caller outside tests until the integration slice wires the render"
)]

use std::sync::Arc;

use core::ffi::{c_double, c_int, c_ulong};

use burrow_types::{Deadline, Error, Result};

use super::{PdfiumDocument, ffi, page_index, raster_dimension, render_progressively, thread};
use crate::pdfsyntax::region::{PageFrame, Region};
use crate::redact::flatten::paint::{
    OPAQUE_BLACK, PageToDevice, PixelRect, bgra_to_rgb, check_ceilings, paint_rectangle,
};
use crate::redact::flatten::{Picture, pixels_for};
use crate::{Annotations, OpenOptions};

/// The flatten render's flags: **`FPDF_ANNOT` (0x01) and nothing else** (§2 rule 9). Pinned by
/// value in a test, because no picture can hold the absence of `FPDF_LCD_TEXT` (0x02): on the
/// pinned PDFium it changes no pixel of this render, with an alpha bitmap or without (measured
/// 2026-10-30). A flag with no visible effect today is still one rule 9 says is not set. The pin
/// holds this constant, and the render passes it as it is; a bit or-ed in at the call site is
/// past the pin, as it is past every picture.
pub(super) const FLAGS: c_int = Annotations::Drawn.render_flags();

// THE TWO DECLARATIONS THIS SLICE ADDS, checked against the pinned
// `engines/vendor/native-*/include/pdfium/fpdfview.h` (the copy the build includes, not `.orig`).
// Same precondition as every declaration in `ffi.rs`: engine thread only.
unsafe extern "C" {
    /// ```c
    /// FPDF_EXPORT FPDF_BOOL FPDF_CALLCONV FPDF_PageToDevice(FPDF_PAGE page, int start_x,
    ///     int start_y, int size_x, int size_y, int rotate, double page_x, double page_y,
    ///     int* device_x, int* device_y);
    /// ```
    /// `fpdfview.h:1066-1075`. "You must make sure the start_x, start_y, size_x, size_y and
    /// rotate parameters have exactly same values as you used in the FPDF_RenderPage() function
    /// call" (`fpdfview.h:1026-1028`). Writes rounded integers, and only on success.
    #[allow(
        clippy::too_many_arguments,
        reason = "one declaration per C function; this one takes ten"
    )]
    fn FPDF_PageToDevice(
        page: ffi::FpdfPage,
        start_x: c_int,
        start_y: c_int,
        size_x: c_int,
        size_y: c_int,
        rotate: c_int,
        page_x: c_double,
        page_y: c_double,
        device_x: *mut c_int,
        device_y: *mut c_int,
    ) -> c_int;

    /// ```c
    /// FPDF_EXPORT FPDF_BOOL FPDF_CALLCONV FPDFBitmap_FillRect(FPDF_BITMAP bitmap, int left,
    ///     int top, int width, int height, FPDF_DWORD color);
    /// ```
    /// `fpdfview.h:1205-1210`. **Declared again here, with the header's `FPDF_BOOL` return**, so
    /// the paint can refuse a fill PDFium reports it did not make. `ffi.rs` declares the same
    /// symbol returning nothing, which the pinned header does not say (filed in this slice's
    /// round 0, not edited here). Both are sound on the ABIs this module builds for -- the int
    /// comes back in a register either way, and the other declaration ignores it -- which is why
    /// the clash is allowed rather than avoided.
    #[allow(
        clashing_extern_declarations,
        reason = "ffi.rs's void return is the clash; this one is the header's (round 0, Q3)"
    )]
    fn FPDFBitmap_FillRect(
        bitmap: ffi::FpdfBitmap,
        left: c_int,
        top: c_int,
        width: c_int,
        height: c_int,
        color: c_ulong,
    ) -> c_int;
}

/// Page *N* at 150 dpi, painted: what the redaction worker is handed (§1 step 5).
#[derive(Debug)]
pub(crate) struct Painted {
    /// *W*, in pixels.
    pub width: u32,
    /// *H*, in pixels.
    pub height: u32,
    /// The pixels, RGB, rows top to bottom, no padding.
    pub rgb: Vec<u8>,
    /// The rectangle the paint filled: reported for tests and the census, **never trusted** by
    /// the cross-check, which computes its own.
    pub painted: PixelRect,
}

impl Painted {
    /// The picture, as the cross-check and the write take it.
    #[must_use]
    pub(crate) fn picture(&self) -> Picture<'_> {
        Picture::new(self.width, self.height, &self.rgb)
    }
}

/// How the paint reaches the engine's transform: production calls it, a test can lie through it.
type Through = fn(&dyn PageToDevice, f64, f64) -> Result<(i32, i32)>;

fn honest(transform: &dyn PageToDevice, x: f64, y: f64) -> Result<(i32, i32)> {
    transform.page_to_device(x, y)
}

/// `FPDF_PageToDevice` for one render: start (0, 0), size *W* x *H*, `rotate = 0`, as
/// `render_page_start` passes them.
struct Transform {
    page: ffi::FpdfPage,
    width: c_int,
    height: c_int,
}

impl PageToDevice for Transform {
    fn page_to_device(&self, page_x: f64, page_y: f64) -> Result<(i32, i32)> {
        let (mut x, mut y): (c_int, c_int) = (0, 0);
        // SAFETY: `page` is live for the whole paint (it is closed after the paint returns, by
        // the job that loaded it), this runs inside that job on the engine thread, and both
        // out-parameters point at live locals. Start, size and rotation are the render's own.
        let ok = unsafe {
            FPDF_PageToDevice(
                self.page,
                0,
                0,
                self.width,
                self.height,
                0,
                page_x,
                page_y,
                &raw mut x,
                &raw mut y,
            )
        };
        if ok == 0 {
            return Err(Error::Internal(
                "pdfium could not place a corner of the region on the page".to_owned(),
            ));
        }
        Ok((x, y))
    }
}

/// [`Transform`] seen through [`Through`].
struct Seen<'t> {
    transform: &'t Transform,
    through: Through,
}

impl PageToDevice for Seen<'_> {
    fn page_to_device(&self, page_x: f64, page_y: f64) -> Result<(i32, i32)> {
        (self.through)(self.transform, page_x, page_y)
    }
}

/// Render page `index` of `source` at 150 dpi over `frame`'s display frame, paint `region` black
/// in PDFium's heap, and return the painted picture as RGB.
///
/// `frame` is page *N*'s frame **as qpdf read it** (`redact::frame::of`), the one the write and
/// the cross-check use.
///
/// # Errors
///
/// - [`Error::LimitExceeded`] for `max_pixels` or `MAX_FLATTEN_PIXELS` on *W*·*H* (until the
///   integration slice gives rule 6 its id), before any allocation; for `max_duration_ms`; and for
///   `max_memory_bytes`, detected, not bounded.
/// - [`Error::Malformed`] for a frame the region conversion refuses, a page PDFium cannot load or
///   draw; [`Error::InvalidArgument`] for a page past the end, or a region wholly off the page.
/// - [`Error::Internal`] for an engine that cannot place, fill or hand back what it drew.
pub(crate) fn render_painted(
    source: &PdfiumDocument,
    index: u64,
    frame: &PageFrame,
    region: &Region,
    options: &OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<Painted> {
    render_painted_through(source, index, frame, region, options, deadline, honest)
}

#[allow(
    clippy::too_many_lines,
    reason = "one engine job, whose every exit must reach the same two releases"
)]
fn render_painted_through(
    source: &PdfiumDocument,
    index: u64,
    frame: &PageFrame,
    region: &Region,
    options: &OpenOptions<'_>,
    deadline: &Deadline,
    through: Through,
) -> Result<Painted> {
    let clock = Arc::clone(&options.clock);
    deadline.checkpoint(clock.as_ref())?;
    frame.check_unit_and_rotation()?;
    frame.check_display_box()?;
    let (display_width, display_height) = frame.displayed_size();
    let (wide, high) = (pixels_for(display_width)?, pixels_for(display_height)?);

    // RULE 6, BEFORE ANYTHING IS ALLOCATED: both ceilings on the product, then the render path's
    // own (`check_pixels`: the 4-byte buffer must be addressable here).
    check_ceilings(wide, high, &options.limits)?;
    crate::raster::check_pixels(wide, high, &options.limits)?;
    crate::estimate::before_page_load(
        source.memory_at_open,
        crate::rss::resident_bytes(),
        &options.limits,
    )?;

    let page_index = page_index(index, source.pages_at_open)?;
    let (w, h) = (raster_dimension(wide)?, raster_dimension(high)?);
    let id = source.id;
    let limits = options.limits;
    let deadline_in = *deadline;
    let clock_in = Arc::clone(&clock);
    let (frame, region) = (*frame, *region);

    let (rgb, painted) = thread::submit(move |registry| {
        let handle = registry.handle(id)?;
        let before = crate::rss::resident_bytes();

        // SAFETY: `handle` came from the registry, so the document is open. Engine thread.
        let page = unsafe { ffi::load_page(handle, page_index) };
        if page.is_null() {
            return Err(Error::Malformed("a page could not be loaded".to_owned()));
        }

        // Everything fallible is inside this closure, so the page is closed on every exit by the
        // single call after it.
        let drawn = (|| -> Result<(Vec<u8>, PixelRect)> {
            // SAFETY: engine thread; plain integers; the ceilings were checked above.
            let bitmap = unsafe { ffi::create_bitmap(w, h, ffi::BITMAP_BGRA) };
            #[cfg(test)]
            created::record(w, h);
            if bitmap.is_null() {
                return Err(Error::Io(
                    "pdfium could not allocate a bitmap for the page".to_owned(),
                ));
            }

            let filled = (|| -> Result<(Vec<u8>, PixelRect)> {
                // SAFETY: `bitmap` is live and `w`/`h` are its extent. A fresh bitmap is undefined.
                unsafe { ffi::fill_bitmap(bitmap, w, h, ffi::OPAQUE_WHITE) };
                render_progressively(
                    bitmap,
                    page,
                    w,
                    h,
                    FLAGS,
                    &deadline_in,
                    clock_in.as_ref(),
                    &limits,
                )?;
                deadline_in.checkpoint(clock_in.as_ref())?;

                // THE PAINT, IN PDFIUM'S HEAP, before a pixel is copied anywhere.
                let transform = Transform {
                    page,
                    width: w,
                    height: h,
                };
                let seen = Seen {
                    transform: &transform,
                    through,
                };
                let rect = paint_rectangle(&region, &frame, (wide, high), &seen)?;
                let side = |v: u32| {
                    c_int::try_from(v).map_err(|_| {
                        Error::Internal("a paint rectangle wider than pdfium takes".to_owned())
                    })
                };
                let (left, top) = (side(rect.left)?, side(rect.top)?);
                let (width, height) = (side(rect.width())?, side(rect.height())?);
                // SAFETY: `bitmap` is live; the rectangle was clamped into its extent by
                // `PixelRect::outward`; engine thread.
                let ok = unsafe {
                    FPDFBitmap_FillRect(
                        bitmap,
                        left,
                        top,
                        width,
                        height,
                        c_ulong::from(OPAQUE_BLACK),
                    )
                };
                if ok == 0 {
                    return Err(Error::Internal(
                        "pdfium did not paint the region it was asked to".to_owned(),
                    ));
                }
                copy_out(bitmap, wide, high).map(|rgb| (rgb, rect))
            })();

            // SAFETY: `bitmap` is live, this is its only destroy, and `copy_out` owns its bytes.
            unsafe { ffi::destroy_bitmap(bitmap) };
            filled
        })();

        // SAFETY: `page` is live, this is its only close, and the document is still open.
        unsafe { ffi::close_page(page) };

        let out = drawn?;
        crate::estimate::check_measured_memory(before, crate::rss::resident_bytes(), &limits)?;
        Ok(out)
    })?;

    deadline.checkpoint(clock.as_ref())?;
    Ok(Painted {
        width: wide,
        height: high,
        rgb,
        painted,
    })
}

/// The bitmap's pixels as RGB, its stride **bounded before a slice of the buffer exists**.
fn copy_out(bitmap: ffi::FpdfBitmap, width: u32, height: u32) -> Result<Vec<u8>> {
    // SAFETY: the caller's bitmap is live; the pointer is owned by it and read before its destroy.
    let (buffer, stride) = unsafe { ffi::bitmap_buffer(bitmap) };
    if buffer.is_null() {
        return Err(Error::Internal(
            "pdfium returned no buffer for a bitmap it allocated".to_owned(),
        ));
    }
    let too_big = || Error::Internal("pdfium's bitmap does not fit in usize".to_owned());
    let stride = usize::try_from(stride)
        .map_err(|_| Error::Internal("pdfium reported a negative stride".to_owned()))?;
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .ok_or_else(too_big)?;
    if stride < row_bytes || stride > row_bytes.saturating_add(row_bytes) {
        return Err(Error::Internal(
            "pdfium reported a stride its own bitmap cannot have".to_owned(),
        ));
    }
    let rows = usize::try_from(height).map_err(|_| too_big())?;
    let span = stride.checked_mul(rows).ok_or_else(too_big)?;
    // SAFETY: `buffer` is non-null and is the bitmap's pixels; `stride` was bounded to
    // `[width * 4, 2 * width * 4]` just above, so `span` cannot exceed what PDFium allocated for a
    // bitmap of these dimensions; the bitmap outlives this borrow (destroyed by the caller after
    // `bgra_to_rgb` returns an owned `Vec`); nothing else on this thread writes it; `u8` has no
    // alignment requirement.
    let src = unsafe { core::slice::from_raw_parts(buffer, span) };
    bgra_to_rgb(src, stride, width, height)
}

/// Every bitmap size this module asked PDFium for, so a test can say none was made before a
/// ceiling refused (ADR 0031 *Fixtures*, rule 6: "check after allocation" must go red).
#[cfg(test)]
mod created {
    use core::ffi::c_int;
    use std::sync::Mutex;

    static SIZES: Mutex<Vec<(c_int, c_int)>> = Mutex::new(Vec::new());

    pub(super) fn record(width: c_int, height: c_int) {
        if let Ok(mut sizes) = SIZES.lock() {
            sizes.push((width, height));
        }
    }

    pub(super) fn any(width: c_int, height: c_int) -> bool {
        SIZES
            .lock()
            .map(|sizes| sizes.contains(&(width, height)))
            .unwrap_or(true)
    }
}

#[cfg(test)]
#[path = "flatten_render_tests.rs"]
mod tests;
