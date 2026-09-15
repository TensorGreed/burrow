import { beforeEach, describe, expect, it } from "vitest";

import { readOrigin } from "./origin-guard.js";

/**
 * The comparison, and the two ways it can be useless.
 *
 * A guard like this fails in one of two directions and neither is visible from a page that
 * happens to be deployed correctly: a comparison that declares everything a match renders it
 * inert, and one that declares everything a mismatch puts a false refusal on every page. So
 * both halves are asserted, and the near-misses are the point of the allow half.
 */

/**
 * THE DOM HALF IS NOT TESTED HERE, AND THAT IS A CHOICE WITH A REASON.
 *
 * vitest runs in node with no DOM, and the two ways to give it one are a jsdom dependency —
 * which `CLAUDE.md` makes a decision rather than a detail, needing a licence audit and a
 * notices entry for a test helper — or a hand-rolled fake. A fake document would be a harness
 * measuring what it can generate: it would confirm that `announce` calls the methods the fake
 * implements, which is not the question. The question is whether a real browser under the
 * real CSP shows the banner, and `e2e/origin-guard.spec.ts` asks a real browser.
 *
 * What is here is the part that is genuinely logic, and the part that can be wrong in a way
 * nobody sees: the comparison.
 */

describe("the origin verdict", () => {
  it("matches when the page is where it was built to be", () => {
    expect(
      readOrigin({ builtFor: "https://burrow.example", servedFrom: "https://burrow.example" }),
    ).toEqual({ kind: "match" });
  });

  it("reports a mismatch, naming both origins and the rebuild that fixes it", () => {
    const verdict = readOrigin({
      builtFor: "https://burrow.pages.dev",
      servedFrom: "https://burrow.example",
    });
    expect(verdict.kind).toBe("mismatch");
    if (verdict.kind !== "mismatch") return;
    // The message has to carry BOTH origins and the action. A message naming only one leaves
    // the reader to guess which half is wrong, and this is read by someone who has just
    // deployed and seen a dead page.
    expect(verdict.message).toContain("https://burrow.pages.dev");
    expect(verdict.message).toContain("https://burrow.example");
    expect(verdict.message).toContain("BURROW_SITE=https://burrow.example");
    // AND IT MUST SAY NOTHING WAS SENT. The person's first thought on seeing a tool fail is
    // about their file, which is the one thing this site promises about.
    expect(verdict.message).toMatch(/nothing has been sent anywhere/i);
  });

  it.each([
    ["a different port is a different origin", "http://localhost:4321", "http://localhost:4322"],
    ["http and https are different origins", "http://burrow.example", "https://burrow.example"],
    ["a subdomain is a different origin", "https://burrow.example", "https://www.burrow.example"],
    ["a custom domain replacing pages.dev", "https://burrow.pages.dev", "https://burrow.example"],
  ])("%s", (_why, builtFor, servedFrom) => {
    expect(readOrigin({ builtFor, servedFrom }).kind).toBe("mismatch");
  });

  it("says 'unknown' rather than 'match' when the build stamped nothing", () => {
    // THE DIRECTION MATTERS. A build without the tag predates this guard or has been
    // stripped; calling that a match would be the guard asserting something it did not
    // check, which is the shape that reads as coverage.
    expect(readOrigin({ builtFor: null, servedFrom: "https://x" })).toEqual({ kind: "unknown" });
    expect(readOrigin({ builtFor: "", servedFrom: "https://x" })).toEqual({ kind: "unknown" });
  });
});
