import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { inkGrid } from "./ink-grid";

/**
 * This TypeScript grid and the Rust one must produce the same sixteen numbers.
 *
 * They are separate implementations by necessity — one runs in a browser, one in a test binary
 * — and a differential corpus whose two sides quantise differently is a corpus comparing its
 * own quantisers. `expectations.json` is the contract between them, so the assertion is
 * against the committed expectation rather than against a second hand-written constant.
 */
describe("the ink grid agrees with the one the corpus records", () => {
  const expectations = JSON.parse(
    readFileSync(
      fileURLToPath(new URL("../../../../tests/conformance/expectations.json", import.meta.url)),
      "utf8",
    ),
  ) as {
    cases: {
      name: string;
      expect: Record<
        string,
        { ok?: { render?: { width: number; height: number; ink_grid: number[] } } }
      >;
    }[];
  };

  const inked = expectations.cases.find((c) => c.name === "render-one-quadrant");

  it("has a corpus case with ink on it to compare against", () => {
    // WITHOUT THIS THE TEST BELOW IS VACUOUS. Every other well-formed fixture in the corpus is
    // structure with no content stream, so its grid is all paper — which an implementation
    // that drew nothing at all would also produce.
    expect(inked, "the corpus has no `render-one-quadrant` case").toBeDefined();
    expect(inked!.expect["render"]?.ok?.render?.ink_grid).toBeDefined();
  });

  it("produces the recorded grid for a raster of the shape that fixture renders to", () => {
    const recorded = inked!.expect["render"]!.ok!.render!;
    const { width, height } = recorded;

    // The fixture is a black rectangle over the top-left quarter of the page, so a correct
    // render is exactly that: ink in the first quarter of each axis, paper everywhere else.
    // Built here rather than rendered, because what is under test is the QUANTISER, not
    // PDFium — the engines are compared by the corpus itself.
    const rgba = new Uint8ClampedArray(width * height * 4);
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const at = (y * width + x) * 4;
        const ink = x < width / 2 && y < height / 2;
        const value = ink ? 0 : 255;
        rgba[at] = value;
        rgba[at + 1] = value;
        rgba[at + 2] = value;
        rgba[at + 3] = 255;
      }
    }

    expect(inkGrid(width, height, rgba)).toEqual(recorded.ink_grid);
  });

  it("quantises at the boundaries it claims, which is where the two can diverge", () => {
    // THE ONLY PLACE THE TWO IMPLEMENTATIONS CAN DISAGREE. Both fixtures above are built from
    // pure 0 and 255, so every cell mean is 0 or 255 and the three thresholds are never
    // touched — code review replaced these thresholds with 10/120/200 and all three tests
    // still passed. Antialiasing on a real render produces exactly the greys in between.
    //
    // The same sixteen values are asserted natively by
    // `core/burrow-ops/tests/render.rs::the_ink_grid_quantises_at_the_boundaries_it_claims`,
    // so a divergence in either direction fails somewhere.
    const flat = (value: number) => {
      const rgba = new Uint8ClampedArray(8 * 8 * 4);
      for (let i = 0; i < rgba.length; i += 4) {
        rgba[i] = value;
        rgba[i + 1] = value;
        rgba[i + 2] = value;
        rgba[i + 3] = 255;
      }
      return inkGrid(8, 8, rgba)[0];
    };

    expect([flat(0), flat(63), flat(64)]).toEqual([3, 3, 2]);
    expect([flat(127), flat(128)]).toEqual([2, 1]);
    expect([flat(191), flat(192)]).toEqual([1, 0]);
    expect(flat(255)).toBe(0);
  });

  it("fills every cell on a raster smaller than the grid", () => {
    // A grid with holes in it compares equal between two implementations that both produced
    // nothing, which is the shape this observable exists to refuse.
    const rgba = new Uint8ClampedArray(2 * 2 * 4);
    expect(inkGrid(2, 2, rgba)).toEqual(new Array(16).fill(3));
  });

  it("sees a quarter turn, which is what makes it an observable rather than a checksum", () => {
    // A readout that reported the same thing for a turned page could not see the operation
    // `/rotate-pdf` exists for. The Rust side asserts this against a really-rotated document;
    // here it is the same claim about the quantiser.
    const width = 64;
    const height = 128;
    const build = (topLeft: boolean) => {
      const rgba = new Uint8ClampedArray(width * height * 4);
      for (let y = 0; y < height; y += 1) {
        for (let x = 0; x < width; x += 1) {
          const at = (y * width + x) * 4;
          const inQuadrant = topLeft
            ? x < width / 2 && y < height / 2
            : x >= width / 2 && y < height / 2;
          const value = inQuadrant ? 0 : 255;
          rgba[at] = value;
          rgba[at + 1] = value;
          rgba[at + 2] = value;
          rgba[at + 3] = 255;
        }
      }
      return rgba;
    };
    expect(inkGrid(width, height, build(true))).not.toEqual(inkGrid(width, height, build(false)));
  });
});
