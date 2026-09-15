import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * The number the islands show a person, held to the number the budget measured.
 *
 * WHY THIS TEST EXISTS. Four tool pages told people the engine download was "about 6.8 MB"
 * for the whole of the PR that took it to about 420 KB — a 16x overstatement of the one
 * quantity that change was about, on the pages whose argument is that every number shown is
 * one we measured. Nothing in the suite pinned it, so nothing said so; the code review did.
 *
 * It is the "fix the control, not the number" rule applied to copy. Correcting the four
 * strings fixes today. This is what makes the next re-measure fix them, because the day
 * `size-budget.json` moves far enough, this fails and names the files.
 *
 * WHAT IT COMPARES. The four `engines/*` lines' `measured_brotli`, summed — what the browser
 * actually pulls over the wire on a first visit, which is what "downloaded" means to a person
 * reading it. NOT `measured_raw`: the old copy quoted the uncompressed figure, which was
 * already overstating the download before PDFium left.
 */

const webApp = resolve(dirname(fileURLToPath(import.meta.url)), "..");

interface BudgetLine {
  measured_raw: number;
  measured_brotli: number;
}
const budget: { artifacts: Record<string, BudgetLine> } = JSON.parse(
  readFileSync(join(webApp, "size-budget.json"), "utf8"),
);

const engineLines = Object.entries(budget.artifacts).filter(([key]) => key.startsWith("engines/"));
const engineBrotli = engineLines.reduce((sum, [, line]) => sum + line.measured_brotli, 0);

/**
 * Every place a person is shown the figure, with the unit it is shown in.
 *
 * A LIST, AND THE COUNT IS ASSERTED. A regex sweep over `src/` would quietly pass the day a
 * fifth tool page arrives with its own wording, which is the "4 of 15 reads as success"
 * failure. Naming the four means a fifth island has to be added here deliberately.
 */
const SHOWN = [
  {
    file: "src/components/MergeTool.svelte",
    pattern: /It is about ([\d,.]+) KB and it is fetched/,
  },
  {
    file: "src/components/RotateTool.svelte",
    pattern: /Starting the PDF engine — about ([\d,.]+) KB, downloaded once/,
  },
  {
    file: "src/components/ReorderTool.svelte",
    pattern: /Starting the PDF engine — about ([\d,.]+) KB, downloaded once/,
  },
  {
    file: "src/components/SplitTool.svelte",
    pattern: /Starting the PDF engine — about ([\d,.]+) KB, downloaded once/,
  },
];

describe("the engine download figure people are shown", () => {
  it("is measured, not remembered — every tool page quotes what the budget recorded", () => {
    expect(engineLines.length, "the budget must have engine lines, or this compares nothing").toBe(
      4,
    );

    const found: string[] = [];
    for (const { file, pattern } of SHOWN) {
      const source = readFileSync(join(webApp, file), "utf8");
      const match = pattern.exec(source);
      expect(
        match,
        `${file} no longer states the engine download in KB, so nobody is checking what it says`,
      ).not.toBeNull();
      const shownKb = Number(match?.[1]?.replace(/,/g, ""));
      const actualKb = engineBrotli / 1000;
      // A 10% band. The copy says "about", and rounding it to a tidy figure is the right thing
      // for prose; what must not survive is a number from a different build.
      expect(
        Math.abs(shownKb - actualKb) / actualKb,
        `${file} says about ${shownKb} KB; the measured engine payload is ${Math.round(actualKb)} KB ` +
          `(${engineBrotli.toLocaleString()} brotli bytes across ${engineLines.length} artifacts). ` +
          `Re-measure and update the copy on all ${SHOWN.length} tool pages.`,
      ).toBeLessThan(0.1);
      found.push(file);
    }
    expect(found, "every tool page that shows the figure was checked").toHaveLength(SHOWN.length);
  });

  it("is the compressed figure, because that is what 'downloaded' means", () => {
    // A guard on the test above rather than on the copy: if someone re-derives the shown
    // number from `measured_raw`, the band above would still pass at some future ratio. The
    // two differ by more than 4x today, and this states which one is the right source.
    const engineRaw = engineLines.reduce((sum, [, line]) => sum + line.measured_raw, 0);
    expect(engineRaw / engineBrotli).toBeGreaterThan(2);
  });
});
