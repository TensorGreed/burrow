//! Instruments I3 and I4: PDFium's text page, and PDFium's raster.
//!
//! **Neither of these is a shape production could adopt as written.** I3 uses `fpdf_text.h`,
//! which has zero references anywhere in this repository outside `engines/vendor/`, and which
//! `tools/check-pdfium-is-render-only.sh` has a three-layer claim about. I4 uses the one-shot
//! `FPDF_RenderPageBitmap`, which ADR 0027's amendment DELETED from the crate in favour of the
//! progressive API, because the one-shot call cannot be checkpointed against a deadline. The
//! spike calls it anyway, because it is measuring what a render costs and not shipping one --
//! and the difference is stated here rather than discovered by a reader.

use std::ffi::CString;
use std::ptr;
use std::sync::Once;

use crate::ffi::*;

static INIT: Once = Once::new();

pub fn init() {
    // PDFium's library init is global and not re-entrant -- `core/CLAUDE.md` records two
    // threads calling it concurrently aborting the process with SIGTRAP. This harness is
    // single-threaded; the `Once` is belt and braces.
    INIT.call_once(|| unsafe { FPDF_InitLibrary() });
}

pub struct Document {
    handle: FPDF_DOCUMENT,
    _bytes: Vec<u8>,
}

pub struct Page {
    handle: FPDF_PAGE,
    pub width: f32,
    pub height: f32,
}

impl Document {
    pub fn open(bytes: Vec<u8>) -> Result<Document, String> {
        init();
        // SAFETY: PDFium does not copy the buffer, so it is moved into the returned
        // Document and dropped only after FPDF_CloseDocument.
        let handle =
            unsafe { FPDF_LoadMemDocument64(bytes.as_ptr().cast(), bytes.len(), ptr::null()) };
        if handle.is_null() {
            // SAFETY: reading the thread-local error code PDFium just set.
            let code = unsafe { FPDF_GetLastError() };
            return Err(format!("FPDF_LoadMemDocument64 failed, error {code}"));
        }
        Ok(Document {
            handle,
            _bytes: bytes,
        })
    }

    pub fn pages(&self) -> i32 {
        // SAFETY: live document handle.
        unsafe { FPDF_GetPageCount(self.handle) }
    }

    pub fn page(&self, index: i32) -> Result<Page, String> {
        // SAFETY: live document handle; an out-of-range index returns null rather than
        // faulting.
        let handle = unsafe { FPDF_LoadPage(self.handle, index) };
        if handle.is_null() {
            return Err(format!("FPDF_LoadPage({index}) returned null"));
        }
        // SAFETY: live page handle.
        let (width, height) = unsafe { (FPDF_GetPageWidthF(handle), FPDF_GetPageHeightF(handle)) };
        Ok(Page {
            handle,
            width,
            height,
        })
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: live handle, closed once.
        unsafe { FPDF_CloseDocument(self.handle) };
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        // SAFETY: live handle, closed once.
        unsafe { FPDF_ClosePage(self.handle) };
    }
}

/// One extracted character, with where PDFium says it is.
#[derive(Debug, Clone)]
pub struct Char {
    pub unicode: char,
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
}

/// I3. Everything PDFium can read off the page as text, decoded THROUGH the font.
pub fn text(page: &Page) -> Result<(String, Vec<Char>), String> {
    // SAFETY: live page handle.
    let tp = unsafe { FPDFText_LoadPage(page.handle) };
    if tp.is_null() {
        return Err("FPDFText_LoadPage returned null".into());
    }
    // SAFETY: live text page.
    let count = unsafe { FPDFText_CountChars(tp) };
    let mut chars = Vec::new();
    let mut text = String::new();
    for i in 0..count {
        // SAFETY: index in range.
        let raw = unsafe { FPDFText_GetUnicode(tp, i) };
        let (mut l, mut r, mut b, mut t) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        // SAFETY: four initialised out-parameters.
        let ok = unsafe { FPDFText_GetCharBox(tp, i, &mut l, &mut r, &mut b, &mut t) };
        let ch = char::from_u32(raw).unwrap_or('\u{fffd}');
        text.push(ch);
        if ok != 0 {
            chars.push(Char {
                unicode: ch,
                left: l,
                right: r,
                bottom: b,
                top: t,
            });
        }
    }
    // SAFETY: closing the text page exactly once, before the page it came from.
    unsafe { FPDFText_ClosePage(tp) };
    Ok((text, chars))
}

pub struct Raster {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// I4. Rasterise the page at `scale` device pixels per point.
pub fn render(page: &Page, scale: f32, flags: i32) -> Result<Raster, String> {
    let w = (page.width * scale).round() as i32;
    let h = (page.height * scale).round() as i32;
    if w <= 0 || h <= 0 {
        return Err("zero-sized page".into());
    }
    // SAFETY: positive dimensions; alpha channel requested, as `raster.rs` does.
    let bitmap = unsafe { FPDFBitmap_Create(w, h, 1) };
    if bitmap.is_null() {
        return Err("FPDFBitmap_Create returned null".into());
    }
    // SAFETY: live bitmap covering the whole surface. Opaque white, matching `ffi.rs:441`.
    unsafe {
        FPDFBitmap_FillRect(bitmap, 0, 0, w, h, 0xFFFF_FFFF);
        // `flags` is 0 for the SHIPPED configuration and FPDF_ANNOT for the second pass.
        // `core/burrow-engines/src/pdfium/ffi.rs:444-450` passes zero deliberately --
        // "annotations are not part of the page" -- and channels 11 and 12 draw their secret
        // through an annotation appearance stream. So the difference between the two passes
        // is the difference between what burrow's render shows and what a viewer shows, which
        // is a measurement rather than a caveat.
        FPDF_RenderPageBitmap(bitmap, page.handle, 0, 0, w, h, 0, flags);
    }
    // SAFETY: live bitmap; buffer and stride are read together, because PDFium pads rows.
    let (buffer, stride) = unsafe {
        (
            FPDFBitmap_GetBuffer(bitmap) as *const u8,
            FPDFBitmap_GetStride(bitmap),
        )
    };
    if buffer.is_null() || stride <= 0 {
        // SAFETY: destroying the bitmap we created.
        unsafe { FPDFBitmap_Destroy(bitmap) };
        return Err("FPDFBitmap_GetBuffer/GetStride refused".into());
    }
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h as usize {
        // SAFETY: row y starts at `stride * y` and is at least 4*w bytes long.
        let row =
            unsafe { std::slice::from_raw_parts(buffer.add(stride as usize * y), w as usize * 4) };
        for x in 0..w as usize {
            let s = &row[x * 4..x * 4 + 4];
            let d = &mut rgba[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4];
            d[0] = s[2]; // B -> R
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
    // SAFETY: destroying the bitmap we created, once, after the last read.
    unsafe { FPDFBitmap_Destroy(bitmap) };
    Ok(Raster {
        width: w,
        height: h,
        rgba,
    })
}

impl Raster {
    /// The fraction of pixels in a PDF-space rectangle that are not background.
    ///
    /// PDF space has y up from the bottom-left; the bitmap has y down from the top-left.
    pub fn ink_in(&self, rect: (f32, f32, f32, f32), page_height: f32, scale: f32) -> f64 {
        let (x0, y0, x1, y1) = rect;
        let px0 = ((x0 * scale).floor().max(0.0)) as usize;
        let px1 = ((x1 * scale).ceil().min(self.width as f32)) as usize;
        let py0 = (((page_height - y1) * scale).floor().max(0.0)) as usize;
        let py1 = (((page_height - y0) * scale).ceil().min(self.height as f32)) as usize;
        if px1 <= px0 || py1 <= py0 {
            return 0.0;
        }
        let mut inked = 0usize;
        let mut total = 0usize;
        for y in py0..py1 {
            for x in px0..px1 {
                let p = &self.rgba[(y * self.width as usize + x) * 4..][..3];
                // Anything visibly off white. Generous on purpose: the question is "is there
                // something there", and a threshold tight enough to miss antialiasing would
                // make a half-erased glyph read as erased.
                if p[0] < 240 || p[1] < 240 || p[2] < 240 {
                    inked += 1;
                }
                total += 1;
            }
        }
        inked as f64 / total as f64
    }

    /// A binary PPM, so a person can look at the page this spike is making claims about.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for chunk in self.rgba.chunks_exact(4) {
            out.extend_from_slice(&chunk[..3]);
        }
        out
    }
}
