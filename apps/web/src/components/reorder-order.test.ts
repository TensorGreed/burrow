import { describe, expect, it } from "vitest";

import { isUnchanged, resolveOrder } from "./reorder-order.js";

const ok = (input: string, pages: number) => {
  const r = resolveOrder(input, pages);
  if (!r.ok) throw new Error(`expected an order, got: ${r.problem}`);
  return r;
};
const problem = (input: string, pages: number) => {
  const r = resolveOrder(input, pages);
  if (r.ok) throw new Error(`expected a refusal, got: ${r.order.join(",")}`);
  return r.problem;
};

describe("resolving a typed page order", () => {
  it("completes a partial order with the pages nobody named", () => {
    // THE RULE THE PAGE STATES, asserted: named pages first in the order given, everything
    // else after in its current order. Without this the tool is unusable for the thing people
    // actually want -- moving one page of a long document.
    expect(ok("3", 5).order).toEqual([3, 1, 2, 4, 5]);
    expect(ok("4, 2", 6).order).toEqual([4, 2, 1, 3, 5, 6]);
  });

  it("keeps the order things were typed in, which is the whole point", () => {
    // A selection is a set and is sorted; an order is a sequence and must not be. `3, 1` and
    // `1, 3` are different documents.
    expect(ok("3, 1", 3).order).toEqual([3, 1, 2]);
    expect(ok("1, 3", 3).order).toEqual([1, 3, 2]);
  });

  it("reads a descending range as a reversal", () => {
    // How "reverse" is expressed. `page-selection.ts` has no use for this -- a set has no
    // direction -- which is why these are two functions and not one.
    expect(ok("5-1", 5).order).toEqual([5, 4, 3, 2, 1]);
    expect(ok("3-1", 5).order).toEqual([3, 2, 1, 4, 5]);
  });

  it("reads an ascending range the ordinary way", () => {
    expect(ok("2-4", 5).order).toEqual([2, 3, 4, 1, 5]);
  });

  it("treats a single-page range as that page", () => {
    expect(ok("2-2", 3).order).toEqual([2, 1, 3]);
  });

  it("is the document's own order when nothing is typed", () => {
    // Not an error. An empty box is somebody who has not typed yet, and the tool disables the
    // button on `isUnchanged` rather than showing a problem nobody caused.
    const r = ok("   ", 4);
    expect(r.order).toEqual([1, 2, 3, 4]);
    expect(r.named).toBe(0);
    expect(isUnchanged(r.order)).toBe(true);
  });

  it("ignores empty parts and any spacing", () => {
    expect(ok(" 3 ,, 1 , ", 4).order).toEqual([3, 1, 2, 4]);
  });

  it("reports how many pages were named, so the page can show the rule working", () => {
    expect(ok("4-2", 6).named).toBe(3);
  });
});

describe("refusing an order that would lose a page", () => {
  it("refuses a page named twice, and says which", () => {
    // THE FAILURE THIS OPERATION EXISTS TO PREVENT. In a selection "1, 1" is one page; in an
    // order it is a document with page 1 twice and another page gone.
    expect(problem("1, 1", 3)).toContain("Page 1 is listed twice");
    expect(problem("2-4, 3", 5)).toContain("Page 3 is listed twice");
    expect(problem("1-3, 3-1", 3)).toContain("listed twice");
  });

  it("refuses a page the document does not have", () => {
    expect(problem("7", 4)).toContain("there is no page 7");
    expect(problem("1-9", 4)).toContain("there is no page 9");
    expect(problem("9-1", 4)).toContain("there is no page 9");
  });

  it("refuses page zero by name, rather than as an out-of-range number", () => {
    // A person typing 0 is off by one, and "numbered from 1" is the answer to that; "there is
    // no page 0" on a 500-page document would be true and useless.
    expect(problem("0", 4)).toContain("numbered from 1");
    expect(problem("0-2", 4)).toContain("numbered from 1");
  });

  it("refuses something that is not a page number, quoting it back", () => {
    expect(problem("two", 4)).toContain('"two" is not a page number');
    expect(problem("1-2-3", 4)).toContain("not a page number");
    expect(problem("-1", 4)).toContain("not a page number");
  });

  it("refuses a document with no pages rather than producing an empty order", () => {
    expect(problem("1", 0)).toContain("no pages");
  });

  it("refuses a paste too long to be an order, before parsing it", () => {
    const huge = Array.from({ length: 9000 }, (_, i) => i + 1).join(",");
    expect(huge.length).toBeGreaterThan(20_000);
    expect(problem(huge, 10)).toContain("longer than this box accepts");
  });

  it("does not expand a huge range before checking it", () => {
    // `1-99999999999` must be a refusal, not eleven billion entries. The endpoints are
    // checked against the document first -- the ordering `burrow_ops::every` needed after
    // `every(1, u64::MAX)` allocated until it panicked.
    const started = Date.now();
    expect(problem("1-99999999999", 10)).toContain("there is no page");
    expect(Date.now() - started).toBeLessThan(1000);
  });
});

describe("recognising an order that changes nothing", () => {
  it("knows the identity", () => {
    expect(isUnchanged([1, 2, 3])).toBe(true);
    expect(isUnchanged([1])).toBe(true);
    expect(isUnchanged([])).toBe(true);
  });

  it("knows anything else", () => {
    expect(isUnchanged([2, 1, 3])).toBe(false);
    expect(isUnchanged([1, 3, 2])).toBe(false);
  });

  it("is what a fully-written-out identity resolves to", () => {
    // Somebody typing `1,2,3` has asked for nothing, and the tool must say so rather than
    // running an operation that rewrites the file to no effect.
    expect(isUnchanged(ok("1,2,3", 3).order)).toBe(true);
    expect(isUnchanged(ok("1-3", 3).order)).toBe(true);
  });
});
