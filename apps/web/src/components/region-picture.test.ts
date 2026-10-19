import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { readRegion } from "./redact-region.js";
import {
  ASPECT_TOLERANCE_PX,
  KEY_START,
  OUTLINE_BORDER_CSS_PX,
  PICTURE_MAX_CSS_WIDTH,
  aspectAgrees,
  fieldsBetween,
  fitInto,
  fractionAt,
  nudged,
  outlineOf,
  outlineSmallerThanBorder,
  PICTURE_COPY,
  arrival,
  pictureBox,
  shownPictureSize,
} from "./region-picture.js";
import { FORBIDDEN_CLAIMS } from "../redact-copy.js";

const LETTER = { width: 612, height: 792 };
const A4 = { width: 595.28, height: 841.89 };
const LANDSCAPE = { width: 792, height: 612 };
const none = { left: "", top: "", width: "", height: "" };
const key = (k: string, shiftKey = false, altKey = false) => ({ key: k, shiftKey, altKey });

describe("the picture's box (#136, S4)", () => {
  it("is the space it has at the screen's density, capped at the track and at DPR 2", () => {
    expect(pictureBox(358, 3)).toEqual({ width: 716, height: 1074 });
    expect(pictureBox(358, 1)).toEqual({ width: 358, height: 537 });
    expect(pictureBox(5000, 2)).toEqual({ width: PICTURE_MAX_CSS_WIDTH * 2, height: 2112 });
  });

  it("stays under LIMITS.maxPixels at its largest, which the core checks before opening", () => {
    const { width, height } = pictureBox(1e9, 1e9);
    expect(width * height).toBeLessThanOrEqual(4 * 1024 * 1024);
  });

  it("falls back to sane numbers rather than asking for a zero or NaN box", () => {
    for (const [css, dpr] of [
      [0, 2],
      [Number.NaN, 2],
      [358, 0],
      [358, Number.NaN],
    ] as const) {
      const box = pictureBox(css, dpr);
      expect(box.width).toBeGreaterThan(0);
      expect(box.height).toBeGreaterThan(0);
    }
  });
});

describe("the aspect guard (#136, S4)", () => {
  const box = { width: 716, height: 1074 };

  it("mirrors the core's fit: the smaller scale, rounded, inside the box", () => {
    expect(fitInto(LETTER, box)).toEqual({ width: 716, height: 927 });
    expect(fitInto(LANDSCAPE, box)).toEqual({ width: 716, height: 553 });
    // Height-limited: a page taller than the box's own shape.
    expect(fitInto({ width: 100, height: 400 }, box)).toEqual({ width: 269, height: 1074 });
  });

  it("agrees with a picture of exactly the frame's fit, and within one pixel a side", () => {
    expect(aspectAgrees({ width: 716, height: 927 }, LETTER, box)).toBe(true);
    expect(aspectAgrees({ width: 716, height: 927 + ASPECT_TOLERANCE_PX }, LETTER, box)).toBe(true);
    expect(aspectAgrees({ width: 716 - 1, height: 927 - 1 }, LETTER, box)).toBe(true);
  });

  it("refuses a picture two pixels off: a different box drawn, so the places differ", () => {
    expect(aspectAgrees({ width: 716, height: 929 }, LETTER, box)).toBe(false);
    expect(aspectAgrees({ width: 714, height: 927 }, LETTER, box)).toBe(false);
  });

  it("refuses a page one reader turned and the other did not", () => {
    // PDFium drew it landscape; the frame says portrait.
    expect(aspectAgrees(fitInto(LANDSCAPE, box), LETTER, box)).toBe(false);
    expect(aspectAgrees(fitInto(LETTER, box), LANDSCAPE, box)).toBe(false);
  });

  it("refuses an empty or nonsensical size rather than dividing by it", () => {
    expect(aspectAgrees({ width: 0, height: 0 }, LETTER, box)).toBe(false);
    expect(aspectAgrees({ width: 716, height: 927 }, { width: 0, height: 792 }, box)).toBe(false);
    expect(aspectAgrees({ width: Number.NaN, height: 927 }, LETTER, box)).toBe(false);
  });
});

describe("a drag on the picture, as the four boxes (#136, S4)", () => {
  const rect = { left: 10, top: 20, width: 306, height: 396 };

  it("reads a pointer as a fraction of the shown picture, clamped to it", () => {
    expect(fractionAt(10, 20, rect)).toEqual({ x: 0, y: 0 });
    expect(fractionAt(316, 416, rect)).toEqual({ x: 1, y: 1 });
    expect(fractionAt(163, 218, rect)).toEqual({ x: 0.5, y: 0.5 });
    expect(fractionAt(-50, 900, rect)).toEqual({ x: 0, y: 1 });
    expect(fractionAt(1, 1, { left: 0, top: 0, width: 0, height: 10 })).toBeNull();
  });

  it("turns two corners into the area between them, in the frame's points", () => {
    expect(fieldsBetween({ x: 0.25, y: 0.5 }, { x: 0.75, y: 0.75 }, LETTER)).toEqual({
      left: "153",
      top: "396",
      width: "306",
      height: "198",
    });
  });

  it("is the same area whichever way it was dragged", () => {
    const a = { x: 0.1, y: 0.9 };
    const b = { x: 0.6, y: 0.2 };
    expect(fieldsBetween(a, b, LETTER)).toEqual(fieldsBetween(b, a, LETTER));
  });

  it("rounds OUTWARD, so the area named always covers the area dragged", () => {
    const a = { x: 0.1234, y: 0.4321 };
    const b = { x: 0.5678, y: 0.8765 };
    const fields = fieldsBetween(a, b, A4)!;
    const read = readRegion(fields, A4);
    expect(read.ok).toBe(true);
    if (!read.ok) return;
    const r = read.region;
    expect(r.left).toBeLessThanOrEqual(a.x * A4.width);
    expect(r.top).toBeLessThanOrEqual(a.y * A4.height);
    expect(r.left + r.width).toBeGreaterThanOrEqual(b.x * A4.width);
    expect(r.top + r.height).toBeGreaterThanOrEqual(b.y * A4.height);
    // And by no more than a tenth of a point at each edge.
    expect(a.x * A4.width - r.left).toBeLessThan(0.1);
    expect(r.left + r.width - b.x * A4.width).toBeLessThan(0.1 + 1e-9);
  });

  it("covers the whole page when dragged corner to corner, even on a page of odd size", () => {
    const fields = fieldsBetween({ x: 0, y: 0 }, { x: 1, y: 1 }, A4)!;
    const read = readRegion(fields, A4);
    expect(read).toEqual({
      ok: true,
      region: { left: 0, top: 0, width: A4.width, height: A4.height },
    });
  });

  it("names nothing for a click with no width or height", () => {
    expect(fieldsBetween({ x: 0.3, y: 0.3 }, { x: 0.3, y: 0.3 }, LETTER)).toBeNull();
    expect(fieldsBetween({ x: 0.3, y: 0.3 }, { x: 0.5, y: 0.3 }, LETTER)).toBeNull();
  });

  it("outlines exactly what the boxes say, through the run's own reading", () => {
    const fields = { left: "153", top: "396", width: "306", height: "198" };
    expect(outlineOf(fields, LETTER)).toEqual({ x: 0.25, y: 0.5, width: 0.5, height: 0.25 });
    expect(outlineOf(none, LETTER)).toBeNull();
    // Boxes the run would refuse draw no outline: the picture never shows an area not sent.
    expect(outlineOf({ ...fields, width: "1000" }, LETTER)).toBeNull();
    expect(outlineOf({ ...fields, left: "1e2" }, LETTER)).toBeNull();
  });
});

describe("the keyboard on the picture (#136, S4)", () => {
  const area = { left: "100", top: "100", width: "72", height: "36" };

  it("starts an area at the top-left with Enter or Space when there is none", () => {
    const expected = {
      left: "0",
      top: "0",
      width: String(KEY_START.width),
      height: String(KEY_START.height),
    };
    expect(nudged(none, LETTER, key("Enter"))).toEqual(expected);
    expect(nudged(none, LETTER, key(" "))).toEqual(expected);
    // And leaves an existing one alone.
    expect(nudged(area, LETTER, key("Enter"))).toBeNull();
  });

  it("moves the area by ten points, or by one with Alt", () => {
    expect(nudged(area, LETTER, key("ArrowRight"))).toEqual({ ...area, left: "110" });
    expect(nudged(area, LETTER, key("ArrowDown", false, true))).toEqual({ ...area, top: "101" });
    expect(nudged(area, LETTER, key("ArrowLeft"))).toEqual({ ...area, left: "90" });
    expect(nudged(area, LETTER, key("ArrowUp"))).toEqual({ ...area, top: "90" });
  });

  it("resizes from the right and bottom edges with Shift", () => {
    expect(nudged(area, LETTER, key("ArrowRight", true))).toEqual({ ...area, width: "82" });
    expect(nudged(area, LETTER, key("ArrowUp", true))).toEqual({ ...area, height: "26" });
  });

  it("stops a move at the page's edge without shrinking the area", () => {
    const atRight = { left: "535", top: "0", width: "72", height: "36" };
    expect(nudged(atRight, LETTER, key("ArrowRight"))).toEqual({ ...atRight, left: "540" });
    const atLeft = { left: "4", top: "0", width: "72", height: "36" };
    expect(nudged(atLeft, LETTER, key("ArrowLeft"))).toEqual({ ...atLeft, left: "0" });
  });

  it("never resizes past the edge or below a tenth of a point", () => {
    const wide = { left: "500", top: "0", width: "100", height: "5" };
    const grown = nudged(wide, LETTER, key("ArrowRight", true))!;
    expect(grown).toEqual({ ...wide, width: "110" });
    // Ten more would end at 620; the page ends at 612.
    expect(nudged(grown, LETTER, key("ArrowRight", true))).toEqual({ ...wide, width: "112" });
    expect(nudged(wide, LETTER, key("ArrowUp", true))).toEqual({ ...wide, height: "0.1" });
  });

  it("every result is a region the run accepts", () => {
    let fields = nudged(none, A4, key("Enter"))!;
    for (const k of ["ArrowRight", "ArrowDown", "ArrowRight", "ArrowDown"]) {
      for (const [shift, alt] of [
        [false, false],
        [true, false],
        [false, true],
        [true, true],
      ] as const) {
        for (let i = 0; i < 80; i += 1) {
          fields = nudged(fields, A4, key(k, shift, alt)) ?? fields;
          expect(readRegion(fields, A4).ok, JSON.stringify(fields)).toBe(true);
        }
      }
    }
  });

  it("leaves every other key to the page, Tab included", () => {
    for (const k of ["Tab", "a", "Escape", "PageDown"]) {
      expect(nudged(area, LETTER, key(k))).toBeNull();
    }
    // And an arrow does nothing with no area to move.
    expect(nudged(none, LETTER, key("ArrowRight"))).toBeNull();
  });
});

describe("a rendered page arriving for the picture (#136, S4; #199, D4)", () => {
  const box = { width: 716, height: 1074 };
  const fit = fitInto(LETTER, box);
  const pixels = (w = fit.width, h = fit.height) => {
    const buffer = new ArrayBuffer(w * h * 4);
    new Uint8Array(buffer).fill(7);
    return buffer;
  };
  const zero = (buffer: ArrayBuffer) => new Uint8Array(buffer).every((b) => b === 0);
  const got = { number: 3, ...fit };

  it("is drawn when it is the page asked for, now, and the shape the frame says", () => {
    const buffer = pixels();
    expect(arrival(buffer, got, { page: 3, current: true }, LETTER, box)).toEqual({
      kind: "draw",
      ...fit,
    });
    expect(zero(buffer), "pixels to be drawn were zeroed").toBe(false);
  });

  it("is zeroed and never drawn when it arrives for something no longer shown", () => {
    const late = pixels();
    expect(arrival(late, got, { page: 3, current: false }, LETTER, box).kind).toBe("stale");
    expect(zero(late)).toBe(true);
    const otherPage = pixels();
    expect(arrival(otherPage, got, { page: 4, current: true }, LETTER, box).kind).toBe("stale");
    expect(zero(otherPage)).toBe(true);
  });

  it("is zeroed and never drawn when the aspect guard refuses it", () => {
    const turned = fitInto(LANDSCAPE, box);
    const buffer = pixels(turned.width, turned.height);
    expect(
      arrival(buffer, { number: 3, ...turned }, { page: 3, current: true }, LETTER, box).kind,
    ).toBe("mismatch");
    expect(zero(buffer)).toBe(true);
  });

  it("is zeroed and never drawn when its bytes are not its stated size", () => {
    const short = new ArrayBuffer(16);
    new Uint8Array(short).fill(7);
    expect(arrival(short, got, { page: 3, current: true }, LETTER, box).kind).toBe("mismatch");
    expect(zero(short)).toBe(true);
  });
});

describe("the picture's words (#136, S4)", () => {
  it("make none of the page's forbidden claims, and never say burrow", () => {
    for (const [name, text] of Object.entries(PICTURE_COPY)) {
      const claims = FORBIDDEN_CLAIMS.filter(({ pattern }) => pattern.test(text)).map(
        (claim) => claim.name,
      );
      expect(claims, `${name}: ${text}`).toEqual([]);
      expect(text, name).not.toMatch(/burrow/i);
    }
  });

  it("say beside the picture that it is not a preview", () => {
    expect(PICTURE_COPY.notPreview).toMatch(/not a preview/);
  });

  it("say what the picture does not draw: form fields and hidden annotations (round 1, SF4)", () => {
    expect(PICTURE_COPY.notPreview).toMatch(/except form fields and hidden annotations/);
  });
});

describe("an area smaller than its outline (#136, S4; security round 1, L-1)", () => {
  const shownSize = { width: 300, height: 400 };
  it("is said to be smaller when it is narrower or shorter on screen than two borders", () => {
    const least = 2 * OUTLINE_BORDER_CSS_PX;
    // Just under two borders wide, and just under two borders high: each on its own.
    expect(outlineSmallerThanBorder({ width: (least - 0.1) / 300, height: 0.5 }, shownSize)).toBe(
      true,
    );
    expect(outlineSmallerThanBorder({ width: 0.5, height: (least - 0.1) / 400 }, shownSize)).toBe(
      true,
    );
  });

  it("is not, at exactly two borders and above (the near-miss)", () => {
    const least = 2 * OUTLINE_BORDER_CSS_PX;
    expect(outlineSmallerThanBorder({ width: least / 300, height: least / 400 }, shownSize)).toBe(
      false,
    );
    expect(outlineSmallerThanBorder({ width: 0.5, height: 0.5 }, shownSize)).toBe(false);
  });
});

/**
 * `RegionPicture.svelte`, with its comments blanked out and offsets kept, as
 * `page-thumbnails.test.ts` reads the strip: a source scan that says so. What needs a browser is
 * `e2e/redact-pdf.spec.ts`'s.
 */
const component = readFileSync(
  fileURLToPath(new URL("./RegionPicture.svelte", import.meta.url)),
  "utf8",
)
  .replace(/\/\*[\s\S]*?\*\//g, (m) => " ".repeat(m.length))
  .replace(/\/\/[^\n]*/g, (m) => " ".repeat(m.length))
  .replace(/<!--[\s\S]*?-->/g, (m) => " ".repeat(m.length));

describe("the picture lets go of its pixels zeroed (#136, S4; #199, D4; round 1, SF2)", () => {
  it("zeroes them in release(), before dropping them", () => {
    expect(component).toMatch(
      /function release\(\) \{\s*zeroPixels\(image\?\.data\);\s*image = null;\s*\}/,
    );
  });

  it("goes through release() at its four sites, and nowhere zeroes or drops by hand", () => {
    // The definition, then exactly four calls: a different file or page, a page arriving over a
    // picture still held, pagehide, and unmount. A fifth site cannot appear without this moving.
    expect([...component.matchAll(/(?<!function )\brelease\(\)/g)].length).toBe(4);
    expect(component).toMatch(/generation \+= 1;\s*release\(\);\s*if \(shownFor && shownFor\.file/);
    expect(component).toMatch(/release\(\);\s*image = new ImageData\(/);
    expect(component).toMatch(
      /const onPageHide = \(\) => \{\s*hidden = true;\s*generation \+= 1;\s*release\(\);/,
    );
    expect(component).toMatch(
      /onDestroy\(\(\) => \{\s*destroyed = true;\s*generation \+= 1;\s*release\(\);/,
    );
    // `image` is set only in release() and right after it, so no picture is dropped unzeroed.
    expect([...component.matchAll(/(?<!let )\bimage = /g)].length, "an assignment to image").toBe(
      2,
    );
    expect(component, "pixels zeroed by hand, outside release()").not.toMatch(/\.fill\(0\)/);
  });

  it("disposes of its host when unmounted mid-render, once the render settles (round 1, nit)", () => {
    expect(component).toMatch(
      /discard: \(\) => \(destroyed \? host\.dispose\(\) : host\.discardWorker\(\)\)/,
    );
    expect(component).toMatch(/if \(paused && !destroyed\) \{/);
  });

  it("draws a re-fed result like any input: no special case, and no words for one (D9, owner 2026-10-19)", () => {
    expect(component).toMatch(/const chosen = file;/);
    expect(component, "a prop that tells a result from any other input").not.toMatch(
      /\bresult\b(?!s)/,
    );
    expect(Object.keys(PICTURE_COPY)).not.toContain("result");
  });

  it("draws the outline with the border outlineSmallerThanBorder assumes", () => {
    expect(component).toMatch(
      new RegExp(`\\.picture__outline \\{[^}]*border: ${OUTLINE_BORDER_CSS_PX}px solid`),
    );
  });
});

describe("the picture's place on screen, reserved before it is drawn (#136, S4)", () => {
  it("is the frame's fit at the box's density, and never wider than the space it had", () => {
    const box = pictureBox(358, 2);
    const shown = shownPictureSize(LETTER, box, 2, 10_000);
    expect(shown.width).toBeLessThanOrEqual(358);
    expect(shown.width / shown.height).toBeCloseTo(LETTER.width / LETTER.height, 2);
  });

  it("is no taller than it is allowed, keeping its shape", () => {
    const shown = shownPictureSize({ width: 100, height: 400 }, pictureBox(704, 1), 1, 300);
    expect(shown.height).toBe(300);
    expect(shown.width).toBeCloseTo(75, 0);
  });
});
