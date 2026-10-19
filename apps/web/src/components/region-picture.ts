// The selection picture's arithmetic (#136, S4): the picture's box, the aspect guard, and the
// conversions between a place on the picture and the four boxes in points.
//
// PURE, AND THE FOUR BOXES STAY THE ONE SOURCE OF TRUTH. A drag or a key writes the boxes; the
// outline on the picture is drawn FROM the boxes, through `readRegion`, so what the run sends is
// always what the boxes say and what the boxes say is always what is outlined. Nothing here
// decides what redacts: the core checks every region it is given (apps/web/CLAUDE.md).
//
// THE FRAME IS `page_frames`', NOT THE PICTURE'S. A region is in the page's display frame --
// points from the top-left corner after `/Rotate` (redact-region.ts) -- and the picture is
// PDFium's drawing of the same page. A place on the picture becomes points as a FRACTION of the
// picture times the frame, so the picture's own scale never matters; what matters is that the
// picture shows the same box as the frame. `aspectAgrees` is the check that it does, as far as
// a picture can say: see its comment for what it cannot see.

import { readRegion, type Region } from "./redact-region.js";

/** The picture is never drawn wider than this in CSS pixels: the page's reading track. */
export const PICTURE_MAX_CSS_WIDTH = 704;

/** A page taller than this many times its width is fitted by height instead. */
export const PICTURE_TALLEST = 1.5;

/** The most device pixels per CSS pixel the picture is drawn at, as a thumbnail's (ADR 0027). */
export const PICTURE_MAX_DPR = 2;

/**
 * The box, in device pixels, the picture is fitted inside: as wide as the space it has (up to
 * the track), at the screen's density up to 2, and `PICTURE_TALLEST` times that high.
 *
 * At most 1408 x 2112 = 2.97 Mpx, under `LIMITS.maxPixels` (4 Mi), which the core checks
 * against the box before the document is opened.
 */
export function pictureBox(
  cssWidth: number,
  devicePixelRatio: number,
): {
  width: number;
  height: number;
} {
  const css = Number.isFinite(cssWidth) && cssWidth > 0 ? cssWidth : PICTURE_MAX_CSS_WIDTH;
  const dpr =
    Number.isFinite(devicePixelRatio) && devicePixelRatio > 0
      ? Math.min(devicePixelRatio, PICTURE_MAX_DPR)
      : 1;
  const width = Math.max(1, Math.round(Math.min(css, PICTURE_MAX_CSS_WIDTH) * dpr));
  return { width, height: Math.max(1, Math.round(width * PICTURE_TALLEST)) };
}

/**
 * The size, in pixels, the core draws a page of `frame` points inside `box`: the same
 * arithmetic as `burrow_ops::render::fit_into` -- the smaller scale, rounded, at least one
 * pixel, never past the box.
 */
export function fitInto(
  frame: { width: number; height: number },
  box: { width: number; height: number },
): { width: number; height: number } {
  const scale = Math.min(box.width / frame.width, box.height / frame.height);
  const width = Math.round(Math.max(frame.width * scale, 1));
  const height = Math.round(Math.max(frame.height * scale, 1));
  return {
    width: Math.max(1, Math.min(width, box.width)),
    height: Math.max(1, Math.min(height, box.height)),
  };
}

/** How far, in pixels per side, a drawn picture may differ from the frame's fit and be used. */
export const ASPECT_TOLERANCE_PX = 1;

/**
 * THE ASPECT GUARD: whether the picture PDFium drew is the shape `page_frames` says the page is.
 *
 * The drawn size must be the frame's own fit into the same box, to within a pixel a side (the
 * two engines round a point size differently, and one pixel of the picture is the most a pointer
 * can choose anyway). A page PDFium turns and the frame reader does not, or a box one reads and
 * the other does not, comes out a different shape -- and then a place on the picture is not that
 * place on the page, so the page offers the numbers only.
 *
 * WHAT IT CANNOT SEE, said plainly: two boxes of the same shape at different places on the page.
 * A picture is the same picture of either. The frame reader clips the crop box to the media box
 * as PDFium does, and refuses the shapes the two read differently (`frame.rs`); this guard is the
 * second line behind that one, not a replacement for it. And a square page turned by 90 degrees
 * in one reader only is square either way.
 */
export function aspectAgrees(
  drawn: { width: number; height: number },
  frame: { width: number; height: number },
  box: { width: number; height: number },
): boolean {
  if (!(drawn.width > 0 && drawn.height > 0 && frame.width > 0 && frame.height > 0)) return false;
  const expected = fitInto(frame, box);
  return (
    Math.abs(drawn.width - expected.width) <= ASPECT_TOLERANCE_PX &&
    Math.abs(drawn.height - expected.height) <= ASPECT_TOLERANCE_PX
  );
}

/** A point on the picture as a fraction of its shown width and height, each 0 to 1. */
export interface Fraction {
  x: number;
  y: number;
}

/** A pointer position inside the shown picture's rectangle, as a `Fraction`, clamped to it. */
export function fractionAt(
  clientX: number,
  clientY: number,
  rect: { left: number; top: number; width: number; height: number },
): Fraction | null {
  if (!(rect.width > 0 && rect.height > 0)) return null;
  const x = (clientX - rect.left) / rect.width;
  const y = (clientY - rect.top) / rect.height;
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
  return { x: clamp01(x), y: clamp01(y) };
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

/** Tenths of a point: the boxes' precision. Integers, so nothing drifts in the arithmetic. */
const TENTHS = 10;

/** The four boxes' text for a region whose edges are whole tenths of a point. */
export interface Fields {
  left: string;
  top: string;
  width: string;
  height: string;
}

/**
 * A page edge in whole tenths, rounded UP: an area that reaches the edge of a 612.35-point page
 * ends at 612.4, which `readRegion` clamps to the edge (it is under a point past it), so the
 * last sliver of the page is never left outside an area that was dragged to the edge.
 */
function edgeTenths(points: number): number {
  return Math.ceil(points * TENTHS);
}

function shown(tenths: number): string {
  // `toFixed(1)` on tenths is exact; a trailing ".0" is dropped, so 72 reads as 72.
  const text = (tenths / TENTHS).toFixed(1);
  return text.endsWith(".0") ? text.slice(0, -2) : text;
}

/**
 * The area between two corners on the picture -- dragged in any direction -- in the frame's
 * points, as the four boxes' text.
 *
 * ROUNDED OUTWARD to the tenth of a point: the left and top edges down, the right and bottom
 * edges up, then clamped to the page. So the area the boxes name always covers the area the
 * person dragged over, never a sliver less of it. `null` for a drag with no width or height.
 */
export function fieldsBetween(
  a: Fraction,
  b: Fraction,
  frame: { width: number; height: number },
): Fields | null {
  const edgeX = edgeTenths(frame.width);
  const edgeY = edgeTenths(frame.height);
  const left = Math.max(0, Math.floor(Math.min(a.x, b.x) * frame.width * TENTHS));
  const top = Math.max(0, Math.floor(Math.min(a.y, b.y) * frame.height * TENTHS));
  const right = Math.min(edgeX, Math.ceil(Math.max(a.x, b.x) * frame.width * TENTHS));
  const bottom = Math.min(edgeY, Math.ceil(Math.max(a.y, b.y) * frame.height * TENTHS));
  if (right <= left || bottom <= top) return null;
  return {
    left: shown(left),
    top: shown(top),
    width: shown(right - left),
    height: shown(bottom - top),
  };
}

/**
 * The region the boxes hold, as fractions of the picture, for drawing its outline -- or `null`
 * when the boxes do not hold a region the page would run (empty, not numbers, past the edge).
 *
 * Through `readRegion`, the same reading the run uses, so the outline is exactly what is sent.
 */
export function outlineOf(
  fields: Fields,
  frame: { width: number; height: number },
): { x: number; y: number; width: number; height: number } | null {
  if ([fields.left, fields.top, fields.width, fields.height].every((t) => t.trim() === "")) {
    return null;
  }
  const reading = readRegion(fields, frame);
  if (!reading.ok) return null;
  const r: Region = reading.region;
  return {
    x: r.left / frame.width,
    y: r.top / frame.height,
    width: r.width / frame.width,
    height: r.height / frame.height,
  };
}

/** A key's effect on the area: move it, or resize it from its right and bottom edges. */
export interface Nudge {
  key: string;
  shiftKey: boolean;
  altKey: boolean;
}

/** One arrow press moves or resizes by this many points; with Alt held, by one. */
export const KEY_STEP_POINTS = 10;

/** An area a key starts when there is none: one inch by half an inch, at the top-left corner. */
export const KEY_START = { width: 72, height: 36 } as const;

/**
 * The boxes after one key press on the picture, or `null` when the key is not one this handles
 * (so the page lets it do what it ordinarily does, Tab included).
 *
 * Arrows MOVE the area; Shift+arrows RESIZE it from the right and bottom edges; Alt makes the
 * step one point instead of ten. Enter or Space with no area starts one at the top-left. The area
 * stays on the page and at least a tenth of a point a side; a move stops at the edge rather than
 * shrinking the area.
 */
export function nudged(
  fields: Fields,
  frame: { width: number; height: number },
  press: Nudge,
): Fields | null {
  const edgeX = edgeTenths(frame.width);
  const edgeY = edgeTenths(frame.height);
  const current = outlineOf(fields, frame);

  if (press.key === "Enter" || press.key === " ") {
    if (current) return null;
    return {
      left: "0",
      top: "0",
      width: shown(Math.min(KEY_START.width * TENTHS, edgeX)),
      height: shown(Math.min(KEY_START.height * TENTHS, edgeY)),
    };
  }

  const direction: Record<string, [number, number]> = {
    ArrowLeft: [-1, 0],
    ArrowRight: [1, 0],
    ArrowUp: [0, -1],
    ArrowDown: [0, 1],
  };
  const step = direction[press.key];
  if (!step || !current) return null;

  // Back to whole tenths: the boxes hold tenths, and `readRegion` clamped any overshoot.
  let left = Math.round(current.x * frame.width * TENTHS);
  let top = Math.round(current.y * frame.height * TENTHS);
  let width = Math.round(current.width * frame.width * TENTHS);
  let height = Math.round(current.height * frame.height * TENTHS);
  const by = (press.altKey ? 1 : KEY_STEP_POINTS) * TENTHS;
  const [dx, dy] = step;

  if (press.shiftKey) {
    width = Math.min(Math.max(1, width + dx * by), edgeX - left);
    height = Math.min(Math.max(1, height + dy * by), edgeY - top);
  } else {
    left = Math.min(Math.max(0, left + dx * by), edgeX - width);
    top = Math.min(Math.max(0, top + dy * by), edgeY - height);
  }
  if (width <= 0 || height <= 0 || left < 0 || top < 0) return null;
  return { left: shown(left), top: shown(top), width: shown(width), height: shown(height) };
}

/** What the page does with one rendered page that arrives for the picture. */
export type Arrival =
  { kind: "draw"; width: number; height: number } | { kind: "stale" } | { kind: "mismatch" };

/**
 * Decide what happens to a rendered page that arrived for the picture, and ZERO ITS PIXELS
 * unless it is to be drawn (#199, D4: rendered pixels are decoded content).
 *
 * - `stale`: it was asked for a document, a page or a showing the picture no longer shows -- a
 *   different file chosen, a different page typed, the page left -- or it is not the page asked
 *   for. Never drawn.
 * - `mismatch`: the aspect guard refused it. Never drawn; the page offers the numbers only.
 * - `draw`: the page shows it, at the size it came out.
 */
export function arrival(
  pixels: ArrayBuffer,
  got: { number: number; width: number; height: number },
  wanted: { page: number; current: boolean },
  frame: { width: number; height: number },
  box: { width: number; height: number },
): Arrival {
  if (!wanted.current || got.number !== wanted.page) {
    new Uint8Array(pixels).fill(0);
    return { kind: "stale" };
  }
  if (!aspectAgrees(got, frame, box) || pixels.byteLength !== got.width * got.height * 4) {
    new Uint8Array(pixels).fill(0);
    return { kind: "mismatch" };
  }
  return { kind: "draw", width: got.width, height: got.height };
}

/**
 * The picture's words. Kept here so `region-picture.test.ts` holds them to the page's forbidden
 * claims (`FORBIDDEN_CLAIMS`) and to "Not Only PDF", never "burrow", as `redact-copy.ts` does.
 */
export const PICTURE_COPY = {
  hint:
    "Drag across the page to choose the area. Or select the picture and press Enter to start an " +
    "area, an arrow key to move it, Shift with an arrow to change its size, and Alt for steps of " +
    "one point. The boxes below show the area in points.",
  notPreview:
    "The picture is the page as a viewer draws it, annotations included. It is not a preview of " +
    "the result.",
  drawing: "Drawing the page…",
  mismatch:
    "The picture of this page did not match the page's measured size, so it is not shown. Type " +
    "the area in points instead.",
  failed: "This page could not be drawn. Type the area in points instead.",
  unavailable: "Page pictures have stopped. Type the area in points instead.",
  // D9 (owner, 2026-10-14): no after-picture of the result. A re-fed result is a result, so it
  // gets no picture, until the owner reads D9 otherwise.
  result: "There is no picture of a result. Type the next area in points.",
} as const;

/**
 * The picture's size on screen, in CSS pixels, KNOWN BEFORE IT IS DRAWN: the frame's fit into
 * the box, at the density the box was taken at, and no taller than `maxCssHeight`.
 *
 * So the page reserves the picture's place the moment a page is chosen, and nothing below it --
 * the four boxes, the run button -- moves when the picture arrives. A control that moves as a
 * person reaches for it is one they press by mistake (measured: a click on the run button landed
 * elsewhere when the picture appeared under the pointer).
 */
export function shownPictureSize(
  frame: { width: number; height: number },
  box: { width: number; height: number },
  devicePixelRatio: number,
  maxCssHeight: number,
): { width: number; height: number } {
  const dpr =
    Number.isFinite(devicePixelRatio) && devicePixelRatio > 0
      ? Math.min(devicePixelRatio, PICTURE_MAX_DPR)
      : 1;
  const fit = fitInto(frame, box);
  let width = fit.width / dpr;
  let height = fit.height / dpr;
  if (Number.isFinite(maxCssHeight) && maxCssHeight > 0 && height > maxCssHeight) {
    width = (width * maxCssHeight) / height;
    height = maxCssHeight;
  }
  return { width, height };
}
