//! Draw pages of a document, and return the pixels.
//!
//! The only operation in this crate that produces pixels rather than a document. It is a
//! **read**, so [ADR 0022](../../../../docs/adr/0022-every-operation-verifies-its-own-output.md)'s
//! read-back does not apply and [`crate::verify::Expected`] gains no variant -- a decision, not
//! an omission, recorded in ADR 0027 §4 together with what it leaves undetectable.
//!
//! # A read, not a write
//!
//! Every other operation in this crate produces a document. This one produces **pixels**, and
//! the difference runs through the whole of it:
//!
//! - [ADR 0022](../../../../docs/adr/0022-every-operation-verifies-its-own-output.md)'s
//!   read-back through a fresh engine does not apply, and [`crate::verify::Expected`] gains no
//!   variant. There is nothing to reopen. What is checked instead is stated in
//!   [ADR 0027](../../../../docs/adr/0027-what-a-render-promises-and-what-it-refuses.md) §4,
//!   and so is the residue: **nothing in the returned pixels identifies which page they came
//!   from.**
//! - the size of the output is a **decision**, not a fact the file stated. That is why
//!   `max_pixels` finally means something here, and why it is the one ceiling read from the
//!   call's own options rather than from the open.
//!
//! # The aspect ratio is decided HERE, in Rust
//!
//! A caller asks for a **box** and each page is fitted inside it, keeping its proportions.
//! The alternative — the caller reads each page's size, does the arithmetic, and asks for an
//! exact width and height — was rejected because on the web that arithmetic would live in
//! JavaScript, and [ADR 0009](../../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md)
//! §2 keeps decisions out of there. It also costs a round trip per page before any pixel is
//! drawn.
//!
//! # One page at a time, whatever the caller asked for
//!
//! The loop is here, and it renders and releases one page before starting the next, so **the
//! engine heap holds at most one bitmap** however long the list is. ADR 0027 §2: this is the
//! answer to [ADR 0020](../../../../docs/adr/0020-rotate-ships-without-thumbnails.md)'s 768 MB,
//! and it is a shape rather than a number — a number can be raised by an edit that looks local.
//!
//! What the *caller* then holds is a different question and a different ceiling, and it is not
//! this crate's: ADR 0027 §3's live window belongs to the page, because only the page knows
//! what is on screen.
//!
//! # Limits
//!
//! | limit | where it applies |
//! |---|---|
//! | `max_input_bytes` | the input, once, at [`Stage::InputSize`] |
//! | `max_pages` | the input's page count, and the number of pages named, at [`Stage::PageCount`] |
//! | `max_pixels` | the **box**, in [`begin`], and each page's raster in the engine, at [`Stage::Pixels`] |
//! | `max_duration_ms` | the whole call, checkpointed **per page** — see the caveat below |
//! | `max_memory_bytes` | **detected, never bounded** (ADR 0007), either side of each render |
//!
//! **THE PER-PAGE CHECKPOINT IS BETWEEN PAGES, NOT INSIDE ONE.** Every other operation's engine
//! call is milliseconds, so `max_duration_ms`' documented "overshoot of up to one engine call"
//! is a rounding error. Here one `FPDF_RenderPageBitmap` on a hostile content stream was
//! measured at **112 seconds and 2.7 GB** while producing a 240x320 thumbnail — `max_pixels`
//! bounds the buffer handed back, not the rasteriser that fills it. ADR 0027 §2a has the
//! figures and names the mechanism that would actually bound it.
//!
//! **The box check subsumes the per-page one, and that is stated rather than left to be
//! discovered.** A page fitted into the box never has more pixels than the box, so once
//! [`begin`] has accepted the box no page of that strip can fail the engine's own
//! `max_pixels` check. The engine keeps it because the engine is also reachable by callers
//! that ask for an exact size rather than a box — the native path, and whatever asks next —
//! and because the check has to sit immediately before the allocation it guards. It is not
//! dead code here; it is simply not the check that fires on this path.
//!
//! **One exception, and it is named rather than rounded off.** [`begin`] calls `Limits::check`
//! directly; the engine calls `raster::check_pixels`, which ALSO refuses a raster whose byte
//! count does not fit a `usize`. On `wasm32` that is a 32-bit `usize`, so a caller who raised
//! `max_pixels` above about 1.07 Gpx could pass a box `begin` accepts and be refused inside the
//! engine. Narrow, and reachable only by a caller setting a ceiling four hundred times the
//! default — but "subsumes" stated absolutely would be an overclaim, and this repository treats
//! that as a bug rather than a wording preference.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_engines::{OpenOptions, PageRenderer, Raster};
use burrow_types::{Clock, Deadline, Error, Limits, Result, Stage};

/// The box each page is drawn to fit inside, in pixels.
///
/// Both dimensions are a **maximum**, not a target: a portrait page in a square box uses the
/// full height and less than the full width. What comes back is therefore not usually the box,
/// which is why every [`Rendered`] carries its own size rather than the caller assuming one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    width: u32,
    height: u32,
}

impl Fit {
    /// A box `width` x `height` pixels.
    ///
    /// Zero in either dimension is refused by [`render`], not here: a `Fit` is just a pair of
    /// numbers, and refusing at the boundary keeps the one refusal in the one place that also
    /// knows the limits.
    #[must_use]
    pub const fn box_of(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// The box's width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The box's height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }
}

/// One page, drawn.
///
/// `Debug` is safe here because [`Raster`]'s is hand-written: formatting one prints its size
/// and the length of its buffer, never the pixels. A derived `Debug` on `Raster` would make
/// `{:?}` on this print somebody's page into a log.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Rendered {
    /// The **one-based** page number that was asked for.
    ///
    /// Carried back rather than left to the caller's own indexing. A strip renders a window of
    /// a document and the results arrive as a list; matching them up by position means a
    /// dropped or reordered element shows the wrong page under the right label, which is
    /// exactly the failure a picture of a page is supposed to prevent.
    pub page: u64,
    /// The pixels, and the size they actually came out at.
    pub raster: Raster,
}

/// Convert a float that has already been range-checked.
///
/// Rust's `as` on a float is saturating rather than undefined, but the workspace denies these
/// casts because a silent saturation on an attacker-influenced number is a real bug class. The
/// check above the cast is what makes this one not that: a page size outside the range is a
/// refusal before it reaches here.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be finite and within u32 on the line before"
)]
fn pixels_from(value: f64) -> Result<u32> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded < 1.0 || rounded > f64::from(u32::MAX) {
        return Err(Error::Internal(
            "a raster dimension fell outside the range that was just checked".to_owned(),
        ));
    }
    Ok(rounded as u32)
}

/// Fit a page `width_pt` x `height_pt` into `fit`, keeping its proportions.
///
/// Returns at least one pixel in each direction: a page 3000 points wide and 1 point tall
/// scaled into a 120-pixel box rounds its height to zero, and a zero-pixel raster is not a
/// small raster — it is a request no engine can satisfy.
///
/// # Errors
///
/// [`Error::Malformed`] if the engine reported a size that is not a size. A page whose
/// `/MediaBox` is degenerate, infinite or NaN is the document's problem, and saying so is
/// better than dividing by it.
fn fit_into(width_pt: f32, height_pt: f32, fit: Fit) -> Result<(u32, u32)> {
    let (w, h) = (f64::from(width_pt), f64::from(height_pt));
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return Err(Error::Malformed(
            "a page does not have a usable size".to_owned(),
        ));
    }

    let scale = (f64::from(fit.width) / w).min(f64::from(fit.height) / h);
    let width = pixels_from((w * scale).max(1.0))?;
    let height = pixels_from((h * scale).max(1.0))?;

    // The rounding can push a dimension one pixel past the box -- a 100.6-point page in a
    // 100-pixel box rounds to 101. Clamping here rather than flooring above keeps the aspect
    // ratio right for every page that is not on the boundary, and keeps the promise the type
    // makes: `Fit` is a maximum.
    Ok((width.min(fit.width).max(1), height.min(fit.height).max(1)))
}

/// A render in progress: the source held open, pages drawn one at a time.
///
/// **The engine is passed to [`Render::next`] rather than held**, exactly as
/// [`crate::split::Split`] does it: holding a reference would put a lifetime on this type, and
/// `wasm_bindgen` cannot export one. The web binding is the caller this shape exists for.
///
/// **ADR 0023's shape, for ADR 0027's reason.** `split` streams its parts because holding
/// fifty documents resident at once is fifty documents resident at once. A strip streams for
/// a sharper reason: [ADR 0027](../../../../docs/adr/0027-what-a-render-promises-and-what-it-refuses.md)
/// §2 says the engine heap holds **at most one bitmap**, and that is only true if the loop
/// lives above the engine and releases each raster before asking for the next.
///
/// Everything that can fail for a reason the caller could have avoided has already happened
/// by the time one of these exists: the page numbers are validated against the real count,
/// the box is checked against `max_pixels`, and the ceilings are applied. A caller holding a
/// `Render` has a request that is known to be answerable.
///
/// **A failure part way through does NOT invalidate the pages already produced**, and that is
/// the one place this differs from `split`. A split is a partition, so a subset of the parts
/// is not a partition of anything; a strip is a set of independent pictures, and a thumbnail
/// that arrived is still a true picture of its page. A caller may keep what it has and report
/// the gap. Said here because the two sessions look alike and the rule is opposite.
pub struct Render<E: PageRenderer> {
    /// The document every page is drawn from, held open for the life of the strip.
    source: E::Document,
    /// The **one-based** page numbers, in the order asked for.
    pages: Vec<u64>,
    fit: Fit,
    /// How many pages have been produced.
    produced: usize,
    /// The whole operation's budget, started after the input was opened.
    deadline: Deadline,
    clock: Arc<dyn Clock>,
    /// The options the strip was begun with, for the per-page `max_pixels` check.
    limits: Limits,
}

impl<E: PageRenderer> Render<E> {
    /// How many pages this strip will produce.
    ///
    /// Known before the first one is, so a caller can lay out the strip and fill it in rather
    /// than growing it as pictures arrive.
    #[must_use]
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// Draw the next page, or `None` when there are none left.
    ///
    /// **One bitmap exists at a time.** The engine allocates, draws, copies out and frees
    /// inside this call, so however long the strip is the engine heap holds one raster's
    /// worth — ADR 0027 §2, and it is the loop being here that makes it true rather than a
    /// number that makes it bounded.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`] — a page has no usable size, or could not be loaded.
    /// - [`Error::LimitExceeded`] — `max_pixels` at [`Stage::Pixels`] before any raster is
    ///   allocated, or the operation's own deadline, checkpointed between pages.
    /// - [`Error::Io`] — the engine could not allocate a bitmap.
    /// - [`Error::Internal`] — the engine returned a raster of a size nobody asked for.
    pub fn next(&mut self, engine: &E, options: &OpenOptions<'_>) -> Option<Result<Rendered>> {
        let page = *self.pages.get(self.produced)?;
        self.produced += 1;
        Some(self.draw(engine, page, options))
    }

    fn draw(&self, engine: &E, page: u64, options: &OpenOptions<'_>) -> Result<Rendered> {
        // PER PAGE, for the reason every other sweep in this crate checkpoints per page: a
        // budget checked only at the ends is one a long enough list walks straight past.
        self.deadline.checkpoint(self.clock.as_ref())?;

        let index = page - 1;
        let (width_pt, height_pt) = engine.page_size(&self.source, index)?;
        let (width, height) = fit_into(width_pt, height_pt, self.fit)?;

        // The ceilings are the ones the strip was BEGUN with, not the ones handed in now.
        // `options` is read for the clock and passed on for the engine's own use; a caller
        // must not be able to raise `max_pixels` between two pages of one strip.
        let mut per_page = OpenOptions::new(self.limits, Arc::clone(&self.clock));
        per_page.password = options.password;

        let raster = engine.render(
            &self.source,
            index,
            width,
            height,
            &per_page,
            &self.deadline,
        )?;

        // The engine already checks the LENGTH, in `Raster::new`, where it cannot be skipped.
        // What is checked here is the pair of numbers: a raster of the right length for the
        // wrong size is the one shape a length check alone cannot see, because 160x80 and
        // 80x160 are both 51,200 pixels.
        if raster.width != width || raster.height != height {
            return Err(Error::Internal(
                "the engine returned a raster of a size nobody asked for".to_owned(),
            ));
        }

        Ok(Rendered { page, raster })
    }
}

/// Open `bytes`, validate the request, and hold the document open for the strip.
///
/// `pages` is **one-based**, because that is how a person names a page and this is the
/// boundary a person's request arrives at. The engine seam is zero-based; the conversion
/// happens once, in [`Render::next`].
///
/// # Errors
///
/// - [`Error::InvalidArgument`] — no page is named; a page number is zero, past the end, or
///   named twice; either dimension of `fit` is zero.
/// - [`Error::Malformed`], [`Error::Unsupported`], [`Error::PasswordRequired`] — the input
///   could not be read.
/// - [`Error::LimitExceeded`] — a ceiling in `options.limits` was reached. `max_pixels` on the
///   **box** fires here, before the document is opened.
pub fn begin<E: PageRenderer>(
    engine: &E,
    bytes: Box<[u8]>,
    pages: &[u64],
    fit: Fit,
    options: &OpenOptions<'_>,
) -> Result<Render<E>> {
    // THE ARGUMENTS BEFORE THE DOCUMENT, like `rotate`: a caller that asked for a zero-sized
    // thumbnail finds out without an untrusted file having been parsed on their behalf.
    if pages.is_empty() {
        return Err(Error::InvalidArgument("no pages to render".to_owned()));
    }
    if fit.width == 0 || fit.height == 0 {
        return Err(Error::InvalidArgument(
            "a render needs a non-zero width and height".to_owned(),
        ));
    }

    let limits = options.limits;
    let clock = Arc::clone(&options.clock);

    // THE BOX ITSELF, against `max_pixels`, before the document is opened. The per-page check
    // in the engine is the one that guards the allocation; this one guards the REQUEST, and it
    // is here for the reason `rotate` checks `max_pages` even though its engine also does -- a
    // ceiling that lives in exactly one place is a ceiling one engine can forget. It is also
    // the only way a caller asking for something impossible is told so before a parse.
    Limits::check(
        Stage::Pixels,
        "max_pixels",
        u64::from(fit.width) * u64::from(fit.height),
        limits.max_pixels,
    )?;

    let source = engine.open(bytes, options)?;
    let total = engine.page_count(&source)?;

    let named = u64::try_from(pages.len())
        .map_err(|_| Error::Internal("page count does not fit in u64".to_owned()))?;
    Limits::check(Stage::PageCount, "max_pages", total, limits.max_pages)?;
    Limits::check(Stage::PageCount, "max_pages", named, limits.max_pages)?;

    // EVERY PAGE NUMBER VALIDATED BEFORE ANY PIXEL IS DRAWN. Half a strip and then a refusal
    // is worse than a refusal: the caller has to decide what to do with the half, and the
    // half it has is indistinguishable from a strip that is still arriving.
    let mut seen = BTreeSet::new();
    for &page in pages {
        if page == 0 {
            return Err(Error::InvalidArgument("page numbers start at 1".to_owned()));
        }
        if page > total {
            return Err(Error::InvalidArgument(format!(
                "page {page} of a {total}-page document"
            )));
        }
        if !seen.insert(page) {
            return Err(Error::InvalidArgument(format!(
                "page {page} is named more than once"
            )));
        }
    }

    let deadline = Deadline::start(clock.as_ref(), &limits);

    Ok(Render {
        source,
        pages: pages.to_vec(),
        fit,
        produced: 0,
        deadline,
        clock,
        limits,
    })
}

/// Draw `pages` of `bytes`, each fitted inside `fit`, and return them all.
///
/// [`begin`] and drain. The convenience form for a caller that wants the whole strip and can
/// hold it — the native tests, the conformance harness, and anything not paying the web's
/// per-message cost. **The web does not use this**: it pulls one page at a time so a strip
/// fills in as it is drawn, and so the main thread never holds the whole set at once.
///
/// # Errors
///
/// [`begin`]'s, plus [`Render::next`]'s. A failure discards the pages already drawn, which is
/// the cost of the convenience: a caller that wants to keep them uses [`begin`].
///
/// # There is no aggregate ceiling here, and that is stated rather than implied
///
/// This holds every raster at once: `pages.len() * max_pixels * 4` bytes, which under
/// `Limits::DEFAULT` is ten thousand pages times 1 GiB. `CLAUDE.md` asks for limits per
/// operation *and in aggregate*, and this form has only the first.
///
/// It is not an attacker's lever — the page count and the box are the CALLER's, not the file's,
/// and a caller who asks for ten thousand full-size rasters gets what they asked for. The web
/// uses [`begin`] precisely so the set is never resident. Said here because a caller reading
/// only this function's signature would have no way to know.
pub fn render<E: PageRenderer>(
    engine: &E,
    bytes: Box<[u8]>,
    pages: &[u64],
    fit: Fit,
    options: &OpenOptions<'_>,
) -> Result<Vec<Rendered>> {
    let mut strip = begin(engine, bytes, pages, fit, options)?;
    let mut out = Vec::with_capacity(strip.pages());
    while let Some(rendered) = strip.next(engine, options) {
        out.push(rendered?);
    }

    // ONE RESULT PER PAGE NAMED, and this is an INVARIANT rather than a check: the loop runs
    // until `next` returns `None`, which is exactly `pages.len()` iterations, so the condition
    // cannot be false. It was written as an `if` returning `Internal`, which reads as a gate and
    // examines nothing -- the shape `CLAUDE.md` names. A debug assertion says what it is.
    debug_assert_eq!(
        out.len(),
        pages.len(),
        "the strip drained to a different length"
    );

    Ok(out)
}
