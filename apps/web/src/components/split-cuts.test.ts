import { describe, expect, it } from "vitest";

import { everyPageCuts, isWholeDocument, partsFrom, resolveCuts } from "./split-cuts.js";

const ok = (input: string, pages: number) => {
  const r = resolveCuts(input, pages);
  if (!r.ok) throw new Error(`expected cuts, got: ${r.problem}`);
  return r;
};
const problem = (input: string, pages: number) => {
  const r = resolveCuts(input, pages);
  if (r.ok) throw new Error(`expected a refusal, got ${r.parts.length} part(s)`);
  return r.problem;
};
/** A part list as `first-last` spans, which is how the page shows them and names them. */
const spans = (input: string, pages: number) =>
  ok(input, pages).parts.map((p) => `${p.first}-${p.first + p.count - 1}`);

describe("resolving a typed cut list", () => {
  it("cuts AFTER the page named, which is the off-by-one people get wrong", () => {
    // `Cuts::after_pages`: [3, 7] on ten pages is 1-3, 4-7, 8-10. If this ever became "cut
    // BEFORE", every part would be wrong by one page and every test below would still pass
    // on shape alone -- so the spans are asserted, not the count.
    expect(spans("3, 7", 10)).toEqual(["1-3", "4-7", "8-10"]);
    expect(ok("3, 7", 10).cuts).toEqual([3, 7]);
  });

  it("makes one more part than there are cuts", () => {
    expect(ok("", 10).parts).toHaveLength(1);
    expect(ok("5", 10).parts).toHaveLength(2);
    expect(ok("2, 4, 6, 8", 10).parts).toHaveLength(5);
  });

  it("covers every page exactly once, which is what makes it a partition", () => {
    // `split` is DEFINED as a partition -- ADR 0023 §3 turns on it, and it is the reason a
    // failed part fails the whole operation. A preview that showed a gap or an overlap would
    // be describing something the core would not produce.
    for (const input of ["", "1", "5", "2, 4, 6, 8", "1, 9"]) {
      const covered = ok(input, 10).parts.flatMap((p) =>
        Array.from({ length: p.count }, (_, i) => p.first + i),
      );
      expect(covered, `cuts ${input || "(none)"}`).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }
  });

  it("treats an empty box as the whole document rather than an error", () => {
    // Somebody who has not typed anything yet is not making a mistake. The PAGE refuses to
    // run this, the way /reorder-pdf refuses an identity; the parser does not.
    const r = ok("   ", 10);
    expect(r.cuts).toEqual([]);
    expect(spans("", 10)).toEqual(["1-10"]);
    expect(isWholeDocument(r.parts)).toBe(true);
    expect(isWholeDocument(ok("5", 10).parts)).toBe(false);
  });

  it("ignores stray separators rather than reading them as pages", () => {
    expect(spans("3,,7", 10)).toEqual(["1-3", "4-7", "8-10"]);
    expect(spans(" 3 , 7 ", 10)).toEqual(["1-3", "4-7", "8-10"]);
  });

  it("refuses a cut after the last page, because that part would be empty", () => {
    // The core refuses it. Saying so beside the box beats a round trip that comes back
    // InvalidArgument, and the message names the last place that WOULD work.
    expect(problem("10", 10)).toContain("after page 9");
    expect(problem("11", 10)).toContain("after page 9");
  });

  it("refuses to cut a one-page document, and says why rather than naming page 0", () => {
    // The general message would read "the last place to cut this document is after page 0",
    // which is nonsense. A one-page document has no gap to cut at.
    expect(problem("1", 1)).toContain("one page");
    expect(problem("1", 1)).not.toContain("page 0");
  });

  it("refuses page 0, since pages are numbered from 1", () => {
    expect(problem("0", 10)).toContain("numbered from 1");
  });

  it("refuses the same cut twice rather than quietly making one part", () => {
    // De-duplicating would be wrong: each cut makes one more part, so `3, 3` asks for
    // something that cannot happen, and silently answering a different question hides it.
    expect(problem("3, 3", 10)).toContain("twice");
  });

  it("refuses cuts out of order rather than sorting them", () => {
    // THE DIFFERENCE FROM `page-selection.ts`, which sorts. A selection is a set and tidying
    // it is kind. A cut list that arrived out of order means the person is thinking about the
    // document differently from the way it will be cut, and sorting hides that.
    expect(problem("7, 3", 10)).toContain("cannot come after");
  });

  it("refuses anything that is not a page number or a range", () => {
    expect(problem("three", 10)).toContain("not a page number");
    expect(problem("3.5", 10)).toContain("not a page number");
    expect(problem("3-", 10)).toContain("not a page number");
  });

  it("reads a range as a cut after each page in it", () => {
    // THE GAP THIS CLOSES. Splitting a 40-page document into single pages needed 39 cut
    // points typed by hand -- 145 characters, and 2,385 for a 500-page one. That is not a
    // tedious way to do a common thing, it is an unreachable one.
    expect(spans("1-3", 10)).toEqual(["1-1", "2-2", "3-3", "4-10"]);
    expect(ok("1-3", 10).cuts).toEqual([1, 2, 3]);
  });

  it("gives every page its own document, which is the case the range exists for", () => {
    expect(spans("1-9", 10)).toEqual([
      "1-1",
      "2-2",
      "3-3",
      "4-4",
      "5-5",
      "6-6",
      "7-7",
      "8-8",
      "9-9",
      "10-10",
    ]);
    expect(ok("1-9", 10).parts).toHaveLength(10);
  });

  it("mixes ranges and single cuts in one list", () => {
    expect(spans("1-2, 7", 10)).toEqual(["1-1", "2-2", "3-7", "8-10"]);
  });

  it("refuses a range that runs backwards rather than reversing it", () => {
    // THE DIFFERENCE FROM `reorder-order.ts`, which reads `9-5` as a reversal because an
    // ORDER has a direction. Cuts are made in order, so a descending range describes nothing
    // a split could do.
    expect(problem("9-5", 10)).toContain("runs backwards");
  });

  it("refuses a range whose end is past the last cuttable page", () => {
    // BOTH ENDPOINTS, BEFORE EXPANDING. `1-99999999999` must be a refusal rather than an
    // allocation -- the ordering `burrow_ops::every` needed after it panicked on a capacity
    // overflow.
    expect(problem("1-10", 10)).toContain("after page 9");
    expect(problem("1-99999999999", 10)).toContain("after page 9");
  });

  it("refuses a range that overlaps what came before it", () => {
    expect(problem("5, 3-7", 10)).toContain("cannot come after");
    expect(problem("3, 3-5", 10)).toContain("twice");
  });

  it("refuses a document with no pages", () => {
    expect(problem("1", 0)).toContain("no pages");
  });

  it("refuses a paste too long to be a cut list, before parsing it", () => {
    expect(problem("1,".repeat(20_000), 10)).toContain("longer than this box accepts");
  });

  it("bounds the work before it allocates, not after", () => {
    // `1-99999999999` as a RANGE is refused as a non-number above; this is the numeric form.
    // The check is against the document, so a huge cut is a refusal rather than an expansion.
    expect(problem("99999999999", 10)).toContain("after page 9");
  });
});

describe("the parts a cut list produces", () => {
  it("is the identity for no cuts", () => {
    expect(partsFrom([], 5)).toEqual([{ first: 1, count: 5 }]);
  });

  it("puts every cut's page at the END of its part", () => {
    // The direction of the off-by-one, asserted on the boundary rather than the shape: a cut
    // after 3 means page 3 is the LAST page of the first part, not the first of the second.
    const parts = partsFrom([3], 5);
    expect(parts[0]).toEqual({ first: 1, count: 3 });
    expect(parts[1]).toEqual({ first: 4, count: 2 });
  });

  it("gives every part at least one page, for any validated cut list", () => {
    for (const cuts of [[1], [9], [1, 2, 3], [2, 4, 6, 8]]) {
      for (const part of partsFrom(cuts, 10)) {
        expect(part.count, `cuts ${cuts.join(",")}`).toBeGreaterThan(0);
      }
    }
  });
});

describe("the every-page preset", () => {
  it("gives a cut list that makes one document per page", () => {
    // It types into the box rather than setting a hidden mode, the way /reorder-pdf's
    // "Reverse the order" does -- so what it did is visible and editable.
    for (const pages of [3, 4, 10, 137]) {
      const parts = ok(everyPageCuts(pages), pages).parts;
      expect(parts, `${pages} pages`).toHaveLength(pages);
      expect(
        parts.every((p) => p.count === 1),
        `${pages} pages`,
      ).toBe(true);
    }
  });

  it("works on a two-page document, where a range would name page 1 twice", () => {
    // `1-{pageCount - 1}` is `1-1` at two pages, which is a legal range; the preset emits the
    // bare `1` instead so the box reads the way a person would write it.
    expect(everyPageCuts(2)).toBe("1");
    expect(ok(everyPageCuts(2), 2).parts).toHaveLength(2);
  });
});
