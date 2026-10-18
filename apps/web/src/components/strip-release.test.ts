import { describe, expect, it } from "vitest";

import { releaseTiles, zeroPixels, type ReleasableTile } from "./strip-release";

/** A tile holding a known picture. */
function tile(state: ReleasableTile["state"], pixels: number[] | null): ReleasableTile {
  return { state, data: pixels ? { data: new Uint8ClampedArray(pixels) } : null };
}

const zero = (a: Uint8ClampedArray) => [...a].every((b) => b === 0);

describe("a tile's pixels are zeroed when the strip lets go of them (#199, D4)", () => {
  // Each reason's states, with `evictedState` passed every time: a `releaseTiles` that applied it
  // whatever the reason, or ignored the reason altogether, gives one of these the wrong states.
  const statesAfter = {
    evict: ["released", "released", "released"],
    document: ["drawn", "drawn", "waiting"],
    pagehide: ["waiting", "waiting", "waiting"],
    unmount: ["drawn", "drawn", "waiting"],
  } as const;
  for (const reason of ["evict", "document", "pagehide", "unmount"] as const) {
    it(`zeroes every tile's pixels on ${reason}, drops them, and gives each tile ${reason}'s state`, () => {
      const tiles = [
        tile("drawn", [9, 8, 7, 6]),
        tile("drawn", [5, 4, 3, 2]),
        tile("waiting", null),
      ];
      const held = tiles.map((t) => t.data?.data ?? null);
      const out = releaseTiles(tiles, reason, "released");
      for (const pixels of held)
        if (pixels) expect(zero(pixels), `${reason} kept a picture`).toBe(true);
      expect(out.every((t) => t.data === null)).toBe(true);
      expect(out.map((t) => t.state)).toEqual(statesAfter[reason]);
    });
  }

  it("gives an evicted tile the caller's state", () => {
    const [out] = releaseTiles([tile("drawn", [1])], "evict", "released");
    expect(out!.state).toBe("released");
  });

  it("puts a drawn tile back to waiting on pagehide, so a restored page draws it again", () => {
    const out = releaseTiles([tile("drawn", [1]), tile("refused", null)], "pagehide");
    expect(out.map((t) => t.state)).toEqual(["waiting", "refused"]);
  });

  for (const reason of ["document", "unmount"] as const) {
    it(`keeps each state on ${reason}, even with an evicted state passed`, () => {
      const out = releaseTiles(
        [tile("drawn", [1]), tile("refused", null), tile("waiting", null)],
        reason,
        "released",
      );
      expect(out.map((t) => t.state)).toEqual(["drawn", "refused", "waiting"]);
    });
  }
});

describe("pixels with nowhere to go are zeroed", () => {
  it("an ArrayBuffer and a typed array alike, and nothing at all is no fault", () => {
    const buffer = new Uint8Array([3, 2, 1]).buffer;
    zeroPixels(buffer);
    expect([...new Uint8Array(buffer)]).toEqual([0, 0, 0]);
    const view = new Uint8ClampedArray([4, 4]);
    zeroPixels(view);
    expect([...view]).toEqual([0, 0]);
    expect(() => zeroPixels(null)).not.toThrow();
  });
});
