// The one region a redaction run takes, typed as numbers (#136, S3; D2: one region per run).
//
// The region is in the page's DISPLAY frame -- points, measured from the top-left corner of the
// page as it is shown, after `/Rotate` and `/UserUnit` -- which is the frame `page_frames` reports
// and `burrow_ops::redact::page` takes. The page checks the four numbers against that frame so the
// problem appears beside the boxes rather than after a round trip; the core still checks
// everything that reaches it (apps/web/CLAUDE.md: the page reports what the core refuses, it does
// not re-implement a ceiling).

export interface Region {
  left: number;
  top: number;
  width: number;
  height: number;
}

export type RegionReading = { ok: true; region: Region } | { ok: false; problem: string };

/** At most this many characters per box, so a pasted essay is refused without being parsed. */
const MAX_FIELD = 32;

/**
 * A number of points as typed: digits with an optional decimal part, nothing else.
 *
 * STRICT, NOT `Number()`. `Number("")` is 0, `Number("1e3")` is 1000 and `Number(" 0x10 ")` is 16,
 * and each of those is a region the person did not type.
 */
function points(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed.length === 0 || trimmed.length > MAX_FIELD) return null;
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return null;
  const value = Number(trimmed);
  return Number.isFinite(value) ? value : null;
}

/**
 * Read the four boxes against the page's frame `[width, height]`, in points.
 *
 * Refused, each with a sentence: a box that is empty or not a plain number; a width or height of
 * zero; a region that starts or runs past the page's edge.
 */
export function readRegion(
  fields: { left: string; top: string; width: string; height: string },
  frame: { width: number; height: number },
): RegionReading {
  const named: [keyof Region, string][] = [
    ["left", "From the left"],
    ["top", "From the top"],
    ["width", "Width"],
    ["height", "Height"],
  ];
  const values: Partial<Region> = {};
  for (const [key, label] of named) {
    const value = points(fields[key]);
    if (value === null) {
      return { ok: false, problem: `${label} needs a number of points, like 72 or 12.5.` };
    }
    values[key] = value;
  }
  const region = values as Region;
  if (region.width <= 0 || region.height <= 0) {
    return { ok: false, problem: "The area needs a width and a height greater than zero." };
  }
  // WHAT THE HINT SHOWS IS ACCEPTED. The page shows the frame rounded down to whole points
  // (`shownSize`); a person who types the size up to the next whole point has typed the edge, so
  // anything under a point past it is clamped to the frame. A point or more past it is refused.
  const width = clamp(region.left, region.width, frame.width);
  const height = clamp(region.top, region.height, frame.height);
  if (width === null || height === null) {
    return {
      ok: false,
      problem: `The area runs past the edge of the page, which is ${shownSize(frame)} points.`,
    };
  }
  return { ok: true, region: { ...region, width, height } };
}

/** How far past an edge a typed area may run and be clamped to it, in points. */
export const EDGE_TOLERANCE = 1;

/** `extent` from `start`, clamped to `edge` if it runs under a point past it; null otherwise. */
function clamp(start: number, extent: number, edge: number): number | null {
  const end = start + extent;
  if (end <= edge) return extent;
  if (end - edge >= EDGE_TOLERANCE) return null;
  const clamped = edge - start;
  return clamped > 0 ? clamped : null;
}

/**
 * The page's size as the page shows it: whole points, rounded DOWN, "W by H".
 *
 * One rendering for the hint and the problem text alike, rounded down so the shown size is never
 * past the frame.
 */
export function shownSize(frame: { width: number; height: number }): string {
  return `${Math.floor(frame.width)} by ${Math.floor(frame.height)}`;
}

/**
 * Every page's frame from a `page_frames` reply, as `{ width, height, refused }`.
 *
 * `frames` is flat `[width, height]` pairs, `0, 0` where a frame rule refused the page, and
 * `frameRefusals` holds that rule per page, or "". A reply whose two arrays disagree in length is
 * not trusted: `null`, so the page reports it as a fault rather than drawing a guess. Two empty
 * arrays are an empty list here; the island treats a document with no pages as unreadable.
 */
export function framesOf(
  frames: readonly number[] | undefined,
  refusals: readonly string[] | undefined,
): { width: number; height: number; refused: string }[] | null {
  if (!frames || !refusals) return null;
  if (frames.length !== refusals.length * 2) return null;
  const out: { width: number; height: number; refused: string }[] = [];
  for (let page = 0; page < refusals.length; page += 1) {
    const width = frames[page * 2] ?? Number.NaN;
    const height = frames[page * 2 + 1] ?? Number.NaN;
    const refused = refusals[page] ?? "";
    if (!Number.isFinite(width) || !Number.isFinite(height)) return null;
    // A page with a frame has a positive size; a refused page has 0, 0. Anything else is a reply
    // this page does not understand.
    if (refused === "" ? width <= 0 || height <= 0 : width !== 0 || height !== 0) return null;
    out.push({ width, height, refused });
  }
  return out;
}
