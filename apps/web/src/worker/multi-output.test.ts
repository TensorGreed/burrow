// ADR 0023's protocol is opt-in, and this is what says so.
//
// The host grows a whole second lifecycle the moment a request posts a `part` message: it
// starts holding outputs, re-arms its watchdog per part, and gates the terminal reply on a
// completeness count. Every one of those is dead weight -- and a new failure mode -- for the
// five operations that deliver one document.
//
// So the claim being asserted is a NEGATIVE one: `merge`, `rotate`, `reorder`, `page_count`
// and `structure_check` post no part and no progress. A test that ran those operations could
// not make it, because the fake would have to reproduce a bug nobody has written yet to fail;
// the durable form is over the source, where the protocol is either reachable from an arm or
// it is not.
//
// It is a source scan and it says so rather than pretending to be behavioural. What it buys is
// that `splitInto` stays the only route to a part message, which is the property the host's
// `partsExpected === undefined` fast path depends on.
//
// SINCE #57 THE CLAIM IS ABOUT TWO BUNDLES, AND THIS SCANNED ONE. `render-main.js` posts
// `progress` messages of its own and a THIRD message shape (`page: {`), and a scan of `main.js`
// alone said nothing about either -- while the header above claimed the protocol was opt-in
// across the worker. A claim about two files checked against one is the shape this file exists
// to prevent, one level up. Found by code review.
//
// A strip's rule is the OPPOSITE of a split's -- pages are delivered as they arrive, because a
// thumbnail that arrived is a true picture of its page whatever happens to page 7 -- so what is
// asserted of `render-main.js` is the same negative in its own terms: `renderStrip` is the only
// route to a `page` message, and nothing else in that bundle posts one.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

const raw = readFileSync(fileURLToPath(new URL("./main.js", import.meta.url)), "utf8");
const rawRender = readFileSync(fileURLToPath(new URL("./render-main.js", import.meta.url)), "utf8");

/** Blank comments, preserving offsets. See `source`. */
function withoutComments(text: string): string {
  return text
    .replace(/\/\*[\s\S]*?\*\//g, (m) => " ".repeat(m.length))
    .replace(/\/\/[^\n]*/g, (m) => " ".repeat(m.length));
}

/**
 * `main.js` with its comments blanked out, offsets preserved.
 *
 * The protocol is DOCUMENTED at the top of `splitInto` in the same shape it is posted in, so a
 * scan of the raw text counts the prose as a call site. Blanking rather than deleting keeps
 * every offset equal to the real file's, so a reported position still points at the line.
 */
const source = withoutComments(raw);

/** The same, for the render bundle's dispatch. */
const renderSource = withoutComments(rawRender);

/**
 * The body of a top-level `function name(...)`, by brace matching.
 *
 * Matched rather than sliced to the next `\nfunction`, because a helper declared inside would
 * end the span early and the scan would then report "no part message here" about a region
 * that does not contain the function at all -- a check that examines nothing.
 */
function functionBody(name: string, text: string = source): string {
  const start = text.indexOf(`function ${name}(`);
  expect(start, `${name} is not declared in the scanned bundle`).toBeGreaterThan(-1);
  const open = text.indexOf("{", start);
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === "{") depth += 1;
    if (text[i] === "}") {
      depth -= 1;
      if (depth === 0) return text.slice(open, i + 1);
    }
  }
  throw new Error(`${name}'s body is unbalanced`);
}

/** Every index at which `needle` occurs. */
function occurrences(haystack: string, needle: string): number[] {
  const found: number[] = [];
  for (let at = haystack.indexOf(needle); at !== -1; at = haystack.indexOf(needle, at + 1)) {
    found.push(at);
  }
  return found;
}

describe("the multi-output protocol is opt-in", () => {
  test("only splitInto posts a part or a progress message", () => {
    const body = functionBody("splitInto");
    const at = source.indexOf(body);

    for (const marker of ["part: {", "progress: {"]) {
      const everywhere = occurrences(source, marker);
      // THE COUNT IS REPORTED AND COMPARED, not merely asserted non-zero: a marker that
      // stopped matching would make "every occurrence is inside splitInto" vacuously true.
      expect(everywhere.length, `no ${marker} in main.js at all -- the scan went blind`).toBe(
        marker === "part: {" ? 1 : 2,
      );
      for (const found of everywhere) {
        expect(
          found >= at && found < at + body.length,
          `${marker} at offset ${found} is outside splitInto`,
        ).toBe(true);
      }
    }
  });

  test("splitInto is reachable from the split arm and from nothing else", () => {
    // Two mentions: the declaration and the one call. A third would be a second operation
    // adopting the protocol without the host's side being reconsidered.
    expect(occurrences(source, "splitInto(").length).toBe(2);

    const declaration = "function splitInto(";
    const call = source.indexOf("splitInto(", source.indexOf(declaration) + declaration.length);
    expect(call, "splitInto is declared and never called").toBeGreaterThan(-1);

    // AND IT IS IN THE SPLIT ARM. Bounded by the arm's own test and the next one, so the
    // assertion is "inside this branch" rather than "somewhere near the word split".
    const arm = source.indexOf('request.op === "split"');
    expect(arm, "no split arm in the dispatch").toBeGreaterThan(-1);
    const next = source.indexOf("request.op === ", arm + 1);
    expect(call).toBeGreaterThan(arm);
    expect(call).toBeLessThan(next);
  });

  // Every operation the dispatch names. `structure_check` is the chain's final `else` rather
  // than a named arm, so it is covered by the case below this one instead of pretended to be
  // here -- an `indexOf` that returned -1 for it would have been the check failing, not the
  // code.
  test.each(["merge", "rotate", "reorder", "page_rotations", "page_count"])(
    "%s posts one terminal reply and no part",
    (op) => {
      // THE ARM ITSELF, from its own test to the next one, rather than a fixed window: a
      // window shorter than the arm reports "no part message here" about a region that is
      // not the whole arm, which is a check that examines less than it claims to.
      const named = source.indexOf(`request.op === "${op}"`);
      expect(named, `${op} is not dispatched in main.js`).toBeGreaterThan(-1);
      const next = source.indexOf("request.op === ", named + 1);
      const arm = source.slice(named, next === -1 ? source.length : next);
      expect(arm.length, `the ${op} arm came out empty`).toBeGreaterThan(20);
      expect(arm).not.toContain("part: {");
      expect(arm).not.toContain("progress: {");
      expect(arm).not.toContain("splitInto(");
    },
  );

  test("structure_check, the chain's final else, posts no part either", () => {
    const last = source.lastIndexOf("request.op === ");
    const arm = source.slice(source.indexOf("} else {", last));
    expect(arm, "the final else is not where structure_check lives").toContain("structure_check(");
    expect(arm).not.toContain("part: {");
    expect(arm).not.toContain("progress: {");
    expect(arm).not.toContain("splitInto(");
  });
});

describe("the render bundle's strip protocol is opt-in too", () => {
  test("only renderStrip posts a page or a progress message", () => {
    const body = functionBody("renderStrip", renderSource);
    const at = renderSource.indexOf(body);

    // `page: {` once, `progress: {` twice -- the total before the first tile and the one after
    // each. THE COUNTS ARE COMPARED, not merely asserted non-zero: a marker that stopped
    // matching would make "every occurrence is inside renderStrip" vacuously true, which is
    // this file's own lesson applied to its second bundle.
    for (const [marker, expected] of [
      ["page: {", 1],
      ["progress: {", 2],
    ] as const) {
      const everywhere = occurrences(renderSource, marker);
      expect(
        everywhere.length,
        `${marker} does not appear in render-main.js as expected -- the scan went blind`,
      ).toBe(expected);
      for (const found of everywhere) {
        expect(
          found >= at && found < at + body.length,
          `${marker} at offset ${found} is outside renderStrip`,
        ).toBe(true);
      }
    }
  });

  test("renderStrip is reachable from the render arm and from nothing else", () => {
    // Two mentions: the declaration and the one call.
    expect(occurrences(renderSource, "renderStrip(").length).toBe(2);

    const declaration = "function renderStrip(";
    const call = renderSource.indexOf(
      "renderStrip(",
      renderSource.indexOf(declaration) + declaration.length,
    );
    expect(call, "renderStrip is declared and never called").toBeGreaterThan(-1);

    const arm = renderSource.indexOf('request.op === "render"');
    expect(arm, "no render arm in the dispatch").toBeGreaterThan(-1);
    expect(call).toBeGreaterThan(arm);
  });

  test("the base bundle posts no page message at all", () => {
    // THE PARTITION, and it is the same argument `check-pdfium-is-render-only.sh` makes about
    // PDFium: an absence rule with no opposite control proves nothing. The strip protocol is
    // the render bundle's, and `main.js` must not have grown one.
    expect(source).not.toContain("page: {");
  });

  test("page_count in the render bundle posts one terminal reply and no page", () => {
    const body = functionBody("runOperation", renderSource);
    const named = body.indexOf('request.op === "render"');
    expect(named, "render is not dispatched in render-main.js").toBeGreaterThan(-1);
    // Everything after the render arm is the `page_count` fall-through.
    const rest = body.slice(body.indexOf("}", named));
    expect(rest, "the page_count fall-through is not where it was").toContain("page_count(");
    expect(rest).not.toContain("page: {");
    expect(rest).not.toContain("renderStrip(");
  });
});
