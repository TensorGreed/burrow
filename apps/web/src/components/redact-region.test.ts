import { describe, expect, it } from "vitest";

import { framesOf, readRegion, shownSize } from "./redact-region.js";

const LETTER = { width: 612, height: 792 };
const typed = (left: string, top: string, width: string, height: string) => ({
  left,
  top,
  width,
  height,
});

describe("the region a run takes, as typed (#136, S3)", () => {
  it("reads four plain numbers inside the page", () => {
    expect(readRegion(typed("72", "100", "200", "14.5"), LETTER)).toEqual({
      ok: true,
      region: { left: 72, top: 100, width: 200, height: 14.5 },
    });
  });

  it("accepts a region that ends exactly on the page's edge, and refuses one point past it", () => {
    expect(readRegion(typed("0", "0", "612", "792"), LETTER).ok).toBe(true);
    const past = readRegion(typed("0", "0", "613", "792"), LETTER);
    expect(past).toEqual({
      ok: false,
      problem: "The area runs past the edge of the page, which is 612 by 792 points.",
    });
    expect(readRegion(typed("1", "0", "612", "792"), LETTER).ok).toBe(false);
    // Under a point past the edge is the edge, clamped; a point past it is refused.
    expect(readRegion(typed("0", "0.5", "612", "792"), LETTER)).toEqual({
      ok: true,
      region: { left: 0, top: 0.5, width: 612, height: 791.5 },
    });
    expect(readRegion(typed("0", "1", "612", "792"), LETTER).ok).toBe(false);
  });

  it("refuses what Number() would have read as something the person did not type", () => {
    for (const text of [
      "",
      " ",
      "1e3",
      "0x10",
      "-5",
      "+5",
      "5.",
      ".5",
      "1,5",
      "Infinity",
      "NaN",
      "5 5",
    ]) {
      const reading = readRegion(typed(text, "0", "10", "10"), LETTER);
      expect(reading.ok, JSON.stringify(text)).toBe(false);
    }
    // The near-miss: surrounding spaces are a typing slip, not a different number.
    expect(readRegion(typed(" 5 ", "0", "10", "10"), LETTER).ok).toBe(true);
  });

  it("refuses a box longer than it will read, without parsing it", () => {
    // 33 characters whose value is IN RANGE (a zero with a long decimal part), so the refusal is
    // the length rule's and not the frame's.
    const long = `0.${"0".repeat(31)}`;
    expect(long).toHaveLength(33);
    expect(readRegion(typed(long, "0", "10", "10"), LETTER)).toEqual({
      ok: false,
      problem: "From the left needs a number of points, like 72 or 12.5.",
    });
    // The near-miss: 32 characters is read.
    expect(readRegion(typed(`0.${"0".repeat(30)}`, "0", "10", "10"), LETTER).ok).toBe(true);
  });

  it("accepts what the hint shows on an A4 page, whose frame is not whole points", () => {
    const A4 = { width: 595.276, height: 841.89 };
    expect(shownSize(A4)).toBe("595 by 841");
    // The size the hint shows, and the next whole point, which is under a point past the edge.
    expect(readRegion(typed("0", "0", "595", "841"), A4).ok).toBe(true);
    expect(readRegion(typed("0", "0", "596", "842"), A4)).toEqual({
      ok: true,
      region: { left: 0, top: 0, width: 595.276, height: 841.89 },
    });
    expect(readRegion(typed("0", "0", "597", "841"), A4)).toEqual({
      ok: false,
      problem: "The area runs past the edge of the page, which is 595 by 841 points.",
    });
  });

  it("refuses a zero width or height, naming the box at fault otherwise", () => {
    expect(readRegion(typed("0", "0", "0", "10"), LETTER)).toEqual({
      ok: false,
      problem: "The area needs a width and a height greater than zero.",
    });
    expect(readRegion(typed("0", "x", "10", "10"), LETTER)).toEqual({
      ok: false,
      problem: "From the top needs a number of points, like 72 or 12.5.",
    });
  });
});

describe("every page's frame, from a page_frames reply", () => {
  it("pairs each page's size with its refusal", () => {
    expect(framesOf([612, 792, 0, 0], ["", "rotate-not-whole"])).toEqual([
      { width: 612, height: 792, refused: "" },
      { width: 0, height: 0, refused: "rotate-not-whole" },
    ]);
  });

  it("does not trust a reply whose arrays disagree or whose sizes contradict the refusal", () => {
    expect(framesOf([612, 792], ["", ""])).toBeNull();
    // Length alone: two pages' sizes for one page's refusals.
    expect(framesOf([612, 792, 0, 0], [""])).toBeNull();
    expect(framesOf([612, 792, 1], ["", ""])).toBeNull();
    expect(framesOf(undefined, [])).toBeNull();
    // A refused page with a size, and an accepted page without one.
    expect(framesOf([612, 792], ["display-box"])).toBeNull();
    expect(framesOf([0, 0], [""])).toBeNull();
    expect(framesOf([Number.NaN, 1], [""])).toBeNull();
    // The near-miss: an empty document is an empty list, not a fault.
    expect(framesOf([], [])).toEqual([]);
  });
});
