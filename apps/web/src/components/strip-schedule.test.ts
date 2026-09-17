import { describe, expect, it } from "vitest";

import {
  LIVE_THUMBNAIL_WINDOW,
  THUMBNAIL_CSS_HEIGHT,
  THUMBNAIL_CSS_WIDTH,
} from "./thumbnail-policy";
import { evictable, outstanding, releasedState, type TileState } from "./strip-schedule";

/**
 * A strip, driven to convergence.
 *
 * The component's own tests are a source scan, and a source scan cannot see a livelock. This
 * runs the scheduler the way `pump()` does — draw everything outstanding, evict as the window
 * fills, repeat — and asserts it stops.
 */
function drive(pages: number, wanted: number[], window: number) {
  const states: TileState[] = Array.from({ length: pages }, () => "waiting");
  const refused = new Set<number>();
  let live: number[] = [];
  let requests = 0;
  let renders = 0;

  while (outstanding(wanted, states, refused).length > 0) {
    requests += 1;
    if (requests > 1_000) break;
    for (const page of outstanding(wanted, states, refused)) {
      while (live.length >= window) {
        const out = evictable(live, wanted, page);
        if (out === null) break;
        live = live.filter((p) => p !== out);
        if (states[out - 1] === "drawn") states[out - 1] = releasedState(out, wanted);
      }
      states[page - 1] = "drawn";
      live.push(page);
      renders += 1;
    }
  }
  return { requests, renders, drawn: states.filter((s) => s === "drawn").length };
}

describe("a strip converges", () => {
  it("draws every tile once when the viewport fits inside the window", () => {
    const wanted = Array.from({ length: 18 }, (_, i) => i + 1); // a 390px phone
    const result = drive(200, wanted, LIVE_THUMBNAIL_WINDOW);
    expect(result.requests).toBe(1);
    expect(result.renders).toBe(18);
    expect(result.drawn).toBe(18);
  });

  it("STOPS when the viewport holds more tiles than the window", () => {
    // THE LIVELOCK, REPLANTED AS AN ASSERTION. Before `released` existed this ran forever:
    // modelled at 112 wanted and a window of 64 it reached 500 requests and 24,064 renders
    // without converging, on a static viewport with nobody scrolling.
    //
    // A window SMALLER than the viewport on purpose, so the property is tested rather than
    // avoided by the constant happening to be big enough today.
    const wanted = Array.from({ length: 112 }, (_, i) => i + 1);
    const result = drive(200, wanted, 64);
    expect(result.requests).toBeLessThanOrEqual(2);
    // Every page is asked for exactly once: the ones the window could not hold are `released`
    // and wait for a scroll rather than being re-requested.
    expect(result.renders).toBe(112);
    expect(result.drawn).toBe(64);
  });

  it("never releases a tile that is on screen while one off screen is available", () => {
    // The ordinary case, and the reason the strip does not blink: eviction prefers a tile the
    // viewport has moved past.
    const live = [1, 2, 3, 4];
    const wanted = [3, 4, 5];
    expect(evictable(live, wanted, 5)).toBe(1);
  });

  it("falls back to the oldest when every live tile is on screen", () => {
    const live = [1, 2, 3];
    const wanted = [1, 2, 3, 4];
    expect(evictable(live, wanted, 4)).toBe(1);
    expect(releasedState(1, wanted)).toBe("released");
  });

  it("sends a tile the viewport has left back to waiting, so it redraws on return", () => {
    expect(releasedState(9, [1, 2, 3])).toBe("waiting");
  });

  it("holds a window at least as large as a 1920x1080 grid, which is what caused this", () => {
    // The policy's own floor: the window may not go below what a viewport shows. It was 64 and
    // a 1920x1080 grid is 112, which is exactly the gap the livelock lived in. `released` makes
    // a smaller window safe rather than fatal; this keeps it from being needed on a desktop.
    const visible = (w: number, h: number) =>
      Math.floor(w / THUMBNAIL_CSS_WIDTH) * Math.ceil(h / THUMBNAIL_CSS_HEIGHT);
    expect(visible(1920, 1080)).toBe(112);
    expect(LIVE_THUMBNAIL_WINDOW).toBeGreaterThanOrEqual(visible(1920, 1080));
  });
});
