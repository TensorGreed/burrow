//! The conformance observable for `render`: a 4x4 grid of ink levels.
//!
//! **Shared rather than written twice**, and that is the whole point of it being here. The
//! native conformance runner computes it, `core/burrow-ops/tests/render.rs` pins its arithmetic
//! against a committed golden, and `apps/web/src/conformance/compare.ts` computes it again in
//! TypeScript — three readers of one definition. Two copies of a quantiser is two quantisers,
//! and the corpus would then be comparing them rather than the engines.
//!
//! # Why a grid rather than a hash
//!
//! Native links `libpdfium.so` and the web loads `pdfium.wasm`, both from `pdfium-binaries` at
//! `chromium/8044` — the same rasteriser on different targets, so antialiasing may differ
//! without anything being wrong. A byte comparison would assert far more than this corpus needs
//! and fail on a difference that means nothing.
//!
//! Dimensions alone cannot see a wrong page; a single "is there ink" cannot see a rotation,
//! which is the case `/rotate-pdf` exists for. Four cells by four, four levels each, sees both
//! and is coarse enough that a build difference does not move it.

// THE DIVISIONS ARE DELIBERATE AND THE REMAINDERS ARE NOT WANTED. `clippy::integer_division`
// exists for attacker-controlled sizes in library code, which is where it stays denied; this
// file divides a known raster into sixteen bands and averages each one. Allowed here rather
// than at eight call sites, because the whole module is that arithmetic.
#![allow(
    clippy::integer_division,
    clippy::cast_possible_truncation,
    reason = "a quantiser: a raster is divided into bands and each band is averaged"
)]

/// Cells per side. Sixteen values in all.
pub const SIDE: usize = 4;

/// A 4x4 grid of ink levels, 0 (paper) to 3 (solid), from a `width` x `height` RGBA raster.
///
/// Luminance is the plain mean of R, G and B — **not** a perceptual weighting. This asks "is
/// there ink here", and a weighting would make the answer depend on the colour of the ink.
///
/// # Panics
///
/// If `rgba` is shorter than `width * height * 4`. Test support: the callers build it from a
/// `Raster`, whose length is checked at construction.
#[must_use]
pub fn ink_grid(width: u32, height: u32, rgba: &[u8]) -> [u8; SIDE * SIDE] {
    let (w, h) = (width as usize, height as usize);
    assert!(
        rgba.len() >= w * h * 4,
        "the raster is shorter than its dimensions"
    );

    let mut grid = [0_u8; SIDE * SIDE];
    for (cell, level) in grid.iter_mut().enumerate() {
        let (cx, cy) = (cell % SIDE, cell / SIDE);
        let (x0, x1) = (cx * w / SIDE, (cx + 1) * w / SIDE);
        let (y0, y1) = (cy * h / SIDE, (cy + 1) * h / SIDE);

        let mut total: u64 = 0;
        let mut count: u64 = 0;
        // `max(y0 + 1)` so a raster with fewer than four pixels on a side still contributes a
        // reading for every cell rather than leaving some empty — a grid with holes in it would
        // compare equal between two implementations that both produced nothing.
        for y in y0..y1.max(y0 + 1) {
            for x in x0..x1.max(x0 + 1) {
                let at = (y.min(h.saturating_sub(1)) * w + x.min(w.saturating_sub(1))) * 4;
                total += u64::from(rgba[at]) + u64::from(rgba[at + 1]) + u64::from(rgba[at + 2]);
                count += 3;
            }
        }
        let mean = total.checked_div(count).unwrap_or(255);
        *level = match mean {
            0..=63 => 3,
            64..=127 => 2,
            128..=191 => 1,
            _ => 0,
        };
    }
    grid
}
