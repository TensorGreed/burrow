import { describe, expect, it } from "vitest";

import { parseSelection } from "./page-selection.js";

/** The pages, or a failure that says what it was. */
function pagesOf(input: string, count = 10): number[] {
  const result = parseSelection(input, count);
  if (!result.ok) throw new Error(`expected ${input} to parse, got: ${result.problem}`);
  return result.pages;
}

/** The problem, or a failure saying it parsed when it should not have. */
function problemOf(input: string, count = 10): string {
  const result = parseSelection(input, count);
  if (result.ok) throw new Error(`expected ${input} to be refused, got ${result.pages}`);
  return result.problem;
}

describe("a page selection", () => {
  it("reads a single page", () => {
    expect(pagesOf("4")).toEqual([4]);
  });

  it("reads a range, inclusive at both ends", () => {
    // Inclusive because a person writing "2-5" means four pages. The off-by-one here would be
    // invisible in the output: a rotation of three pages where four were asked for still
    // produces a valid document.
    expect(pagesOf("2-5")).toEqual([2, 3, 4, 5]);
  });

  it("reads a mixed list in any order, with any spacing", () => {
    expect(pagesOf("7-9, 1,4")).toEqual([1, 4, 7, 8, 9]);
    expect(pagesOf("  3 ,  1  ")).toEqual([1, 3]);
  });

  it("sorts and de-duplicates", () => {
    // The core refuses a page named twice, and "1, 1" from a person means one page rather
    // than a mistake worth refusing. Overlapping ranges are the common way this happens.
    expect(pagesOf("3,1,3")).toEqual([1, 3]);
    expect(pagesOf("1-3, 2-4")).toEqual([1, 2, 3, 4]);
  });

  it("reads a range that is one page", () => {
    expect(pagesOf("5-5")).toEqual([5]);
  });

  it("reads the whole document when asked for it explicitly", () => {
    expect(pagesOf("1-10")).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  });

  it("refuses an empty selection rather than treating it as everything", () => {
    // The tool has a separate control meaning "every page". A blank box quietly meaning the
    // same thing is how somebody rotates a whole document they meant to sample.
    expect(problemOf("")).toContain("Type which pages");
    expect(problemOf("   ")).toContain("Type which pages");
  });

  it("refuses a page past the end, and says how many there are", () => {
    expect(problemOf("11", 10)).toContain("10 pages");
    expect(problemOf("8-12", 10)).toContain("past the end");
  });

  it("says 'page' rather than 'pages' for a one-page document", () => {
    expect(problemOf("2", 1)).toContain("1 page,");
  });

  it("refuses page zero, because pages are numbered from one", () => {
    // The core refuses it too. Refusing here as well means the person hears about it without
    // a round trip -- and the two refusals agreeing is what the conformance corpus is for.
    expect(problemOf("0")).toContain("numbered from 1");
    expect(problemOf("0-3")).toContain("numbered from 1");
  });

  it("refuses a backwards range rather than swapping it", () => {
    // Swapping is the tempting fix and the wrong one: "5-2" is a typo, and a tool that
    // reinterprets what you typed is one you cannot check.
    expect(problemOf("5-2")).toContain("runs backwards");
    expect(problemOf("5-2")).toContain("2-5");
  });

  it("refuses something that is not a number, and says which part", () => {
    // ASSERTED PER INPUT, because the messages are the product. `not.toBe("")` carried almost
    // no information -- the real assertion was `problemOf`'s own throw. Found by code review.
    const cases: [string, string][] = [
      ["two", "not a page number"],
      ["1-two", "not a page number"],
      ["4.5", "not a page number"],
      ["1e3", "not a page number"],
      ["-3", "not a page number"],
      ["3-", "not a page number"],
      // Arabic-Indic digits. `\d` in JavaScript is ASCII-only, so these are not numbers here
      // -- which is the right answer, and worth pinning: a future `/u` flag or a
      // "be helpful about unicode" change would silently start accepting them.
      ["٤", "not a page number"],
      ["1-2-3", "not a page or a range"],
    ];
    for (const [input, expected] of cases) {
      expect(problemOf(input), `"${input}"`).toContain(expected);
    }
  });

  it("refuses an empty item rather than skipping it", () => {
    // "1,,3" is a typo. Skipping it would silently accept a list the person did not write.
    expect(problemOf("1,,3")).toContain("empty item");
    expect(problemOf("1,")).toContain("empty item");
    expect(problemOf(",1")).toContain("empty item");
  });

  it("refuses a number too long to compare exactly", () => {
    // Ten digits and up: past this, `Number` comparisons stop being exact and the range check
    // below would be approximate. Refused rather than clamped.
    expect(problemOf("1234567890")).toContain("not a page number");
  });

  it("does not expand a huge range before checking it", () => {
    // THE ALLOCATION ORDER, which is the one thing here that could take the tab down rather
    // than merely be wrong. `burrow_ops::every` allocated until it panicked with a capacity
    // overflow for exactly this shape; the fix there and here is to check both endpoints
    // before expanding. If this ever regresses, the test does not fail -- it hangs.
    const started = Date.now();
    expect(problemOf("1-999999999", 10)).toContain("past the end");
    expect(Date.now() - started).toBeLessThan(1_000);
  });

  it("refuses an implausibly long input", () => {
    expect(problemOf("1,".repeat(2_000))).toContain("a lot of text");
  });

  it("refuses anything at all on a document with no pages", () => {
    // Unreachable through the tool, which never offers a selection for a document it could
    // not open -- and the parser is a pure function that does not get to assume its caller.
    expect(problemOf("1", 0)).toContain("no pages");
  });
});
