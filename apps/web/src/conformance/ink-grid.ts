/**
 * The conformance observable for `render`, in TypeScript.
 *
 * **A port of `core/burrow-engines/testsupport/ink_grid.rs`, and the two are held together by
 * `ink-grid.test.ts`** — which asserts this produces the same sixteen numbers the corpus
 * records for the one fixture with ink on it. Two copies of a quantiser is two quantisers, and
 * the corpus would then be comparing those rather than the engines.
 *
 * It lives here rather than in the harness driver so it can be tested without a browser. The
 * harness returns raw pixels; the grid is computed on this side.
 *
 * # Why a grid rather than a hash
 *
 * Native links `libpdfium.so` and the web loads `pdfium.wasm`, both from `pdfium-binaries` at
 * `chromium/8044` — the same rasteriser on different targets, so antialiasing may differ
 * without anything being wrong. Dimensions alone cannot see a wrong page; a single "is there
 * ink" cannot see a rotation, which is the case `/rotate-pdf` exists for.
 */

/** Cells per side. Sixteen values in all. */
export const SIDE = 4;

/**
 * A 4x4 grid of ink levels, 0 (paper) to 3 (solid), from a `width` x `height` RGBA raster.
 *
 * Luminance is the plain mean of R, G and B — **not** a perceptual weighting. This asks "is
 * there ink here", and a weighting would make the answer depend on the colour of the ink.
 */
export function inkGrid(width: number, height: number, rgba: ArrayLike<number>): number[] {
  if (rgba.length < width * height * 4) {
    throw new Error("the raster is shorter than its dimensions");
  }

  const grid: number[] = [];
  for (let cell = 0; cell < SIDE * SIDE; cell += 1) {
    const cx = cell % SIDE;
    const cy = Math.floor(cell / SIDE);
    const x0 = Math.floor((cx * width) / SIDE);
    const x1 = Math.floor(((cx + 1) * width) / SIDE);
    const y0 = Math.floor((cy * height) / SIDE);
    const y1 = Math.floor(((cy + 1) * height) / SIDE);

    let total = 0;
    let count = 0;
    // `max(y0 + 1)` so a raster with fewer than four pixels on a side still contributes a
    // reading for every cell rather than leaving some empty — a grid with holes in it would
    // compare equal between two implementations that both produced nothing.
    for (let y = y0; y < Math.max(y1, y0 + 1); y += 1) {
      for (let x = x0; x < Math.max(x1, x0 + 1); x += 1) {
        const at = (Math.min(y, height - 1) * width + Math.min(x, width - 1)) * 4;
        total += rgba[at]! + rgba[at + 1]! + rgba[at + 2]!;
        count += 3;
      }
    }
    const mean = count === 0 ? 255 : Math.floor(total / count);
    grid.push(mean <= 63 ? 3 : mean <= 127 ? 2 : mean <= 191 ? 1 : 0);
  }
  return grid;
}
