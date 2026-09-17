import { describe, expect, it } from "vitest";

import { LIMITS } from "./tool-host";
import {
  LIVE_THUMBNAIL_WINDOW,
  MAX_DEVICE_PIXEL_RATIO,
  THUMBNAIL_CSS_HEIGHT,
  THUMBNAIL_CSS_WIDTH,
  devicePixelRatio,
  thumbnailBox,
  windowBytes,
} from "./thumbnail-policy";

/**
 * ADR 0027's arithmetic, held to the code that implements it.
 *
 * The ADR quotes these figures in prose to argue that the numbers it CHOSE are safe. A figure
 * quoted in prose and computed nowhere is a figure that goes stale the first time somebody
 * edits a constant — and this record is one whose whole point is that it can be revised.
 */
describe("the thumbnail policy ADR 0027 chose", () => {
  it("draws a tile at 240x320 device pixels on a DPR-2 screen", () => {
    expect(thumbnailBox(2)).toEqual({ width: 240, height: 320 });
  });

  it("costs the 307,200 bytes a tile the ADR quotes", () => {
    const box = thumbnailBox(2);
    expect(box.width * box.height * 4).toBe(307_200);
  });

  it("holds about 19.7 MB for a full window, 26x below the 512 MiB recycle threshold", () => {
    expect(windowBytes(2)).toBe(19_660_800);
    // ADR 0015 §5's threshold, and the ratio stated as a number rather than as "orders of
    // magnitude". THE FIRST DRAFT SAID "two orders below" AND THIS TEST DISPROVED IT: at 32
    // tiles the window was 9.8 MB, which is 55x below 512 MiB, not 100x. A round phrase that
    // is 45% wrong is the overclaiming-doc-comment bug the root CLAUDE.md names, and it was
    // in the ADR before it was here.
    const recycleThreshold = 512 * 1024 * 1024;
    expect(windowBytes(2) * 26).toBeLessThan(recycleThreshold);
  });

  it("quarters to 76,800 bytes a tile at DPR 1, which is revision point 3's whole argument", () => {
    const box = thumbnailBox(1);
    expect(box.width * box.height * 4).toBe(76_800);
    expect(windowBytes(1) * 4).toBe(windowBytes(2));
    // THE ABSOLUTE, because the DPR revision point's prose quotes it in two records and only
    // the ratio was pinned. It said "2.5 MB" -- the 32-tile figure -- after the window became
    // 64, so the two records disagreed and nothing computed either.
    expect(windowBytes(1)).toBe(4_915_200);
  });

  it("caps the ratio rather than following it", () => {
    expect(devicePixelRatio(3)).toBe(MAX_DEVICE_PIXEL_RATIO);
    expect(devicePixelRatio(1.5)).toBe(1.5);
  });

  it("falls back to 1 for every ratio a browser will not give us", () => {
    // A headless context, a stubbed window, a hostile override. A thumbnail drawn at NaN
    // device pixels is a canvas of nothing, and it would fail silently.
    for (const bad of [undefined, 0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(devicePixelRatio(bad)).toBe(1);
    }
  });

  it("cannot reach the pixel ceiling through the strip, at any ratio", () => {
    // THE RELATIONSHIP THE ADR PROMISES: a person cannot hit `max_pixels` by looking at
    // thumbnails. The ceiling guards our own code and other callers of the core, and this is
    // the assertion that keeps that true if either number is revised.
    for (const ratio of [undefined, 1, 1.5, 2, 3, 10]) {
      const box = thumbnailBox(ratio);
      expect(box.width * box.height).toBeLessThan(LIMITS.maxPixels);
    }
    // 1/54th of it at the cap, which is the figure the ADR quotes.
    const box = thumbnailBox(2);
    expect(Math.floor(LIMITS.maxPixels / (box.width * box.height))).toBe(54);
  });

  it("keeps a window larger than a phone or a 1280px desktop shows, and says where it stops", () => {
    // THE FLOOR ON REVISING IT, computed rather than asserted in prose. A window smaller than
    // the viewport blanks tiles while somebody is looking at them.
    //
    // This test is why the window is 64 and not 32: the ADR's first draft claimed a 1280px
    // desktop shows "about 27" tiles, and the arithmetic says 50.
    const visible = (width: number, height: number) =>
      Math.floor(width / THUMBNAIL_CSS_WIDTH) * Math.ceil(height / THUMBNAIL_CSS_HEIGHT);

    expect(visible(390, 844)).toBe(18);
    expect(visible(1280, 800)).toBe(50);
    expect(LIVE_THUMBNAIL_WINDOW).toBeGreaterThan(visible(390, 844));
    expect(LIVE_THUMBNAIL_WINDOW).toBeGreaterThan(visible(1280, 800));

    // AND WHERE IT STOPS, asserted so the residual cannot quietly stop being stated: a
    // 1920x1080 grid shows more than the window, so on that screen tiles at the far edge are
    // redrawn on scroll. A desktop is the machine best able to absorb that, which is why the
    // number is not raised to cover it.
    // THE EXACT FIGURE, not just "more than the window". The prose in two records quotes it,
    // and an unpinned number in prose is the shape the "27 tiles" claim already cost.
    expect(visible(1920, 1080)).toBe(112);
    expect(visible(1920, 1080)).toBeGreaterThan(LIVE_THUMBNAIL_WINDOW);
  });
});

describe("the pixel ceiling the page asks for", () => {
  it("is 4 Mpx, not the core's 256 Mpx default", () => {
    expect(LIMITS.maxPixels).toBe(4_194_304);
  });

  it("admits a full page at 200 dpi and refuses one at 300", () => {
    // The two rows of ADR 0027's table that the number was chosen between. A4 in points is
    // 595.28 x 841.89; at 200 dpi that is 1653 x 2339, at 300 dpi 2479 x 3508.
    expect(1653 * 2339).toBeLessThan(LIMITS.maxPixels);
    expect(2479 * 3508).toBeGreaterThan(LIMITS.maxPixels);
  });

  it("refuses the PDF maximum page at 1:1, which the core's default accepts", () => {
    // THE FINDING THAT DECIDED THE NUMBER. 14400 x 14400 points is 207 Mpx and 791 MiB, and
    // `Limits::DEFAULT.max_pixels` (256 Mpx) passes it. If this ever stops being true the ADR
    // is wrong about why the default could not be the render ceiling.
    const pdfMaximumPage = 14_400 * 14_400;
    expect(pdfMaximumPage).toBe(207_360_000);
    expect(pdfMaximumPage).toBeLessThan(256 * 1024 * 1024);
    expect(pdfMaximumPage).toBeGreaterThan(LIMITS.maxPixels);
  });
});
