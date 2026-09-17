// The strip's withdrawal and the prose describing it are checked against each other.
//
// `STRIP_WITHDRAWN` is one boolean that decides what two pages DO and what four pages SAY.
// Without this file, flipping it back would leave four paragraphs asserting the opposite of
// the interface — and nothing would fail, because prose is not type-checked and nobody reads
// `/credits` before shipping. The person restoring the strip is exactly the person who will
// not remember which paragraphs to rewrite.
//
// Two halves, and both are needed:
//
//   * the rule against the BUILT HTML of all four routes, gated on the number of pages
//     examined rather than on nothing failing;
//   * the rule against fixtures it must accept and near-misses it must reject, so a run that
//     examined nothing cannot read like a run that examined everything.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { PRODUCTION_DIR, REPO } from "../build-output.js";
import {
  CREDITS_PRESENT_MARK,
  CREDITS_WITHDRAWN_MARK,
  PAGES_THAT_DESCRIBE_THE_STRIP,
  PRESENT_MARK,
  REORDER_PRESENT_MARK,
  REORDER_WITHDRAWN_MARK,
  STRIP_WITHDRAWN,
  WITHDRAWN_MARK,
} from "./strip-copy.js";

/** The mark a page must carry, and the one it must not, for the current flag. */
function expected(page: string): { present: string; absent: string } {
  // THREE PAIRS, NOT ONE. `/credits` talks about licence components rather than about a strip,
  // and `/reorder-pdf` says "no page pictures here YET" in BOTH states -- so the shared
  // withdrawn mark is a substring of its mounted copy, and a single pair passed whatever that
  // page said. Caught by flipping the flag and rebuilding.
  const [withdrawn, mounted] = page.startsWith("credits/")
    ? [CREDITS_WITHDRAWN_MARK, CREDITS_PRESENT_MARK]
    : page.startsWith("reorder-pdf/")
      ? [REORDER_WITHDRAWN_MARK, REORDER_PRESENT_MARK]
      : [WITHDRAWN_MARK, PRESENT_MARK];
  return STRIP_WITHDRAWN
    ? { present: withdrawn, absent: mounted }
    : { present: mounted, absent: withdrawn };
}

/**
 * Astro emits its own scoped-style attributes and re-wraps text at the source's line breaks,
 * so the marks are matched against text with runs of whitespace collapsed rather than against
 * the raw markup. A mark that spans a line break in the `.astro` source would otherwise be
 * absent from a page that says it.
 */
function flattened(page: string): string {
  return readFileSync(join(PRODUCTION_DIR, page), "utf8").replace(/\s+/g, " ");
}

describe("the rule itself", () => {
  // Pointed at fixtures before it is pointed at the build. A rule nobody has seen fail is a
  // rule nobody knows can fail, and this one guards prose that no type checker touches.

  it("accepts a page carrying the mark for the current state", () => {
    const { present } = expected("split-pdf/index.html");
    expect(`<p><strong>${present}.</strong> and then some prose.</p>`).toContain(present);
  });

  it("rejects a page carrying the mark for the other state", () => {
    const { present, absent } = expected("split-pdf/index.html");
    const wrong = `<p><strong>${absent}.</strong> and then some prose.</p>`;
    expect(wrong).not.toContain(present);
    expect(wrong).toContain(absent);
  });

  it("uses a different pair of marks for the pages whose sentence is not the shared one", () => {
    // The credits paragraph is about licence components rather than about a strip, so it has
    // its own pair. A single pair reused there would pass whatever that page said.
    expect(expected("credits/index.html").present).not.toBe(
      expected("split-pdf/index.html").present,
    );
    expect(expected("reorder-pdf/index.html").present).not.toBe(
      expected("split-pdf/index.html").present,
    );

    // AND THE REORDER PAIR MUST NOT BE A SUBSTRING TRAP. Its mounted copy contains the shared
    // withdrawn mark, which is the bug this pair exists for; assert that directly so the pair
    // cannot be "simplified" back later.
    expect(REORDER_PRESENT_MARK).not.toContain(REORDER_WITHDRAWN_MARK);
    expect(WITHDRAWN_MARK).not.toBe(REORDER_WITHDRAWN_MARK);
  });
});

describe("the pages that describe the strip", () => {
  it("say what the flag says, and nothing of the other state", () => {
    for (const page of PAGES_THAT_DESCRIBE_THE_STRIP) {
      const html = flattened(page);
      const { present, absent } = expected(page);
      expect(html, `${page} must say: ${present}`).toContain(present);
      expect(html, `${page} must not still say: ${absent}`).not.toContain(absent);
    }

    // THE COUNT IS THE MEASUREMENT. Four routes describe this feature; a list that quietly
    // shrank to three would pass every assertion above while leaving a page lying.
    expect(PAGES_THAT_DESCRIBE_THE_STRIP).toHaveLength(4);
  });

  it("branch on the constant rather than hard-coding whichever state is current", () => {
    // The assertion above passes if a page happens to contain the right words today. That is
    // not the property wanted: the page must DERIVE them, so flipping the flag rewrites the
    // page instead of leaving it to somebody's memory. Checked against the sources, since the
    // built HTML cannot show where its words came from.
    const sources = [
      "src/pages/split-pdf.astro",
      "src/pages/rotate-pdf.astro",
      "src/pages/reorder-pdf.astro",
      "src/pages/credits.astro",
    ];
    for (const source of sources) {
      const text = readFileSync(join(REPO, "apps", "web", source), "utf8");
      expect(text, `${source} must branch on STRIP_WITHDRAWN`).toContain("STRIP_WITHDRAWN");
    }
    expect(sources).toHaveLength(PAGES_THAT_DESCRIBE_THE_STRIP.length);
  });

  it("mount the strip only when it is not withdrawn", () => {
    // The other half of the same agreement: what the pages SAY has to match what the islands
    // DO. Both islands gate the component on the flag, so a page cannot advertise pictures it
    // does not draw, or stay silent about ones it does.
    for (const island of ["src/components/SplitTool.svelte", "src/components/RotateTool.svelte"]) {
      const text = readFileSync(join(REPO, "apps", "web", island), "utf8");
      expect(text, `${island} must gate the strip on the flag`).toMatch(
        /\{#if !STRIP_WITHDRAWN\}\s*<PageThumbnails/,
      );
    }
  });
});
