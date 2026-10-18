import { describe, expect, it } from "vitest";

import {
  END_DEADLINE_MS,
  RENDER_BATCH,
  batchOf,
  createEndGate,
  discardStale,
} from "./strip-lifecycle";

/** A gate over a fake clock: `advance` fires every timer due by then. */
function gate() {
  let now = 0;
  let discards = 0;
  const timers = new Map<number, { at: number; fn: () => void }>();
  let next = 1;
  const g = createEndGate({
    discard: () => {
      discards += 1;
    },
    setTimer: (fn, ms) => {
      const id = next++;
      timers.set(id, { at: now + ms, fn });
      return id;
    },
    clearTimer: (id) => {
      timers.delete(id as number);
    },
  });
  const advance = (ms: number) => {
    now += ms;
    for (const [id, t] of [...timers]) {
      if (t.at <= now) {
        timers.delete(id);
        t.fn();
      }
    }
  };
  return { g, advance, discards: () => discards, timers: () => timers.size };
}

describe("the strip's render worker ends one document after another (#199, D1)", () => {
  it("ends at once when nothing is rendering", () => {
    const t = gate();
    t.g.request(false);
    expect(t.discards()).toBe(1);
    expect(t.timers(), "no deadline is armed for an end that already happened").toBe(0);
  });

  it("waits for a render in flight, and ends the moment it settles", () => {
    const t = gate();
    t.g.request(true);
    expect(t.discards(), "terminated mid-render").toBe(0);
    expect(t.g.pending()).toBe(true);
    t.advance(END_DEADLINE_MS - 1);
    expect(t.discards()).toBe(0);
    t.g.settled();
    expect(t.discards(), "the end was not carried out when the render settled").toBe(1);
    // AND ONCE: the deadline is cancelled, so it cannot end a later document's worker.
    t.advance(END_DEADLINE_MS * 2);
    expect(t.discards()).toBe(1);
    expect(t.g.pending()).toBe(false);
  });

  it("ends anyway when the render in flight has not settled by the deadline", () => {
    const t = gate();
    t.g.request(true);
    t.advance(END_DEADLINE_MS);
    expect(t.discards(), "a hung render kept the previous document's worker alive").toBe(1);
    // The render settling late ends nothing further.
    t.g.settled();
    expect(t.discards()).toBe(1);
  });

  it("asks once however often an end is asked for while waiting", () => {
    const t = gate();
    t.g.request(true);
    t.g.request(true);
    expect(t.timers()).toBe(1);
    t.g.settled();
    expect(t.discards()).toBe(1);
  });

  it("a settle with no end asked for ends nothing", () => {
    const t = gate();
    t.g.settled();
    expect(t.discards()).toBe(0);
  });

  it("a cancel drops a waiting end and its deadline, ending nothing", () => {
    const t = gate();
    t.g.request(true);
    t.g.cancel();
    expect(t.g.pending()).toBe(false);
    expect(t.timers(), "the deadline outlived the cancel").toBe(0);
    t.advance(END_DEADLINE_MS * 2);
    t.g.settled();
    expect(t.discards(), "a cancelled end was carried out").toBe(0);
  });

  it("does not kill a slow render that is still producing pages (owner, 2026-10-16)", () => {
    // Eight pages at 2.5 s each: 20 s in all, past the deadline many times over, but never 3 s
    // without a page.
    const t = gate();
    t.g.request(true);
    for (let page = 0; page < 8; page += 1) {
      t.advance(END_DEADLINE_MS - 500);
      expect(t.discards(), `killed while page ${page + 1} was arriving`).toBe(0);
      t.g.progress();
    }
    expect(t.timers(), "progress left more than one deadline armed").toBe(1);
    t.g.settled();
    expect(t.discards(), "the end was not carried out when the render settled").toBe(1);
  });

  it("kills a silent render at the deadline, counted from its last page", () => {
    const t = gate();
    t.g.request(true);
    t.advance(2_000);
    t.g.progress();
    t.advance(END_DEADLINE_MS - 1);
    expect(t.discards(), "the deadline ran from the request, not the last page").toBe(0);
    t.advance(1);
    expect(t.discards(), "a render silent for END_DEADLINE_MS kept its worker").toBe(1);
  });

  it("a page with no end waiting arms nothing", () => {
    const t = gate();
    t.g.progress();
    expect(t.timers()).toBe(0);
    t.advance(END_DEADLINE_MS * 2);
    expect(t.discards()).toBe(0);
  });

  it("is a few seconds, said as a number", () => {
    expect(END_DEADLINE_MS).toBe(3_000);
  });
});

describe("an end waits for at most one batch (owner, 2026-10-16)", () => {
  it("asks for at most RENDER_BATCH pages, in order, and the rest on the next round", () => {
    expect(RENDER_BATCH).toBeGreaterThanOrEqual(1);
    expect(RENDER_BATCH).toBeLessThanOrEqual(4);
    const waiting = Array.from({ length: 40 }, (_, i) => i + 1);
    expect(batchOf(waiting)).toEqual(waiting.slice(0, RENDER_BATCH));
    expect(batchOf([7, 3])).toEqual([7, 3]);
    expect(batchOf([])).toEqual([]);
  });
});

describe("a render that arrives for a document no longer shown is zeroed (D4)", () => {
  it("zeroes every byte of its pixels", () => {
    const pixels = new Uint8Array([9, 8, 7, 6, 5]).buffer;
    discardStale(pixels);
    expect([...new Uint8Array(pixels)]).toEqual([0, 0, 0, 0, 0]);
  });
});
