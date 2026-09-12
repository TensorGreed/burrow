// The first-load size budget, asserted against a real production build.
//
// ROADMAP item 11: "record the module size and fail on an unexplained regression". Two words
// there do the work.
//
// **Module size is not the thing to budget.** What a user pays is the sum of everything
// fetched before their first operation can run: the page shell, the worker bundle, all three
// .wasm modules, and the CSP control file. Budgeting them individually lets three files each
// grow 4% -- a 4% regression -- while every per-file budget passes. So the total is the
// binding gate here and the per-artifact lines exist to say where it went. There is a test
// below that plants exactly that distributed regression and asserts the total catches it.
//
// **"Unexplained" is what the budget file is for.** `size-budget.json` records the measured
// value beside the budget, so raising one means editing a number next to the measurement it
// came from and saying why in the commit. A budget that is raised without anyone knowing what
// grew has stopped being a budget.
//
// Brotli, because that is what a host serves. Raw bytes are recorded and reported -- they are
// what the browser compiles, and what spike 0001 measured -- but they are not gated: a change
// that made the module compress better while getting larger on disk is not a regression a
// user experiences.
//
// The build comes from `vitest.global-setup.ts`, with no `BURROW_HARNESS` set. Measuring a
// harness build would count `host/` and the harness route, which no deploy ships.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { byBudgetKey, firstLoad } from "../../../tools/first-load.mjs";
import { PRODUCTION_DIR } from "./build-output.js";

const webApp = resolve(dirname(fileURLToPath(import.meta.url)), "..");

interface Line {
  measured_raw: number;
  measured_brotli: number;
  budget_brotli: number;
  why?: string;
}

const budget: {
  artifacts: Record<string, Line>;
  total: Line;
} = JSON.parse(readFileSync(join(webApp, "size-budget.json"), "utf8"));

const measurement = firstLoad(PRODUCTION_DIR);
const groups = byBudgetKey(measurement);

const kb = (n: number) => `${(n / 1024).toFixed(1)} KiB`;

describe("the first-load size budget", () => {
  it("stays within the total budget", () => {
    // THE GATE. Everything else in this file is diagnosis.
    expect(
      measurement.total.brotli,
      `first load is ${kb(measurement.total.brotli)} brotli, over the ` +
        `${kb(budget.total.budget_brotli)} budget. Find what grew in the per-artifact ` +
        `failures below; do not raise this number without knowing`,
    ).toBeLessThanOrEqual(budget.total.budget_brotli);
  });

  it("stays within every per-artifact budget", () => {
    const over: string[] = [];
    for (const [key, line] of Object.entries(budget.artifacts)) {
      const actual = groups[key];
      if (!actual) continue; // covered by the coverage test below
      if (actual.brotli > line.budget_brotli) {
        over.push(
          `${key}: ${kb(actual.brotli)} > ${kb(line.budget_brotli)} ` +
            `(was ${kb(line.measured_brotli)} when the budget was set)`,
        );
      }
    }
    expect(over, `over budget:\n  ${over.join("\n  ")}`).toEqual([]);
  });

  it("budgets every artifact the payload actually contains", () => {
    // Without this, a NEW engine artifact -- a third .wasm, a second bundle -- would be
    // counted in the total and budgeted by nothing, and the first person to notice would be
    // whoever raised the total to make CI pass.
    const unbudgeted = Object.keys(groups).filter((key) => !(key in budget.artifacts));
    expect(
      unbudgeted,
      `first load contains artifacts with no budget: ${unbudgeted.join(", ")}. ` +
        `Add them to size-budget.json with a measured value and a reason`,
    ).toEqual([]);
  });

  it("has no budget line for an artifact that no longer ships", () => {
    // The other direction. A stale line is a budget nothing can ever trip, and a reader
    // would take it for coverage.
    const stale = Object.keys(budget.artifacts).filter((key) => !(key in groups));
    expect(stale, `size-budget.json budgets files that do not ship: ${stale.join(", ")}`).toEqual(
      [],
    );
  });

  it("measures a payload that is actually the shipped one", () => {
    // A budget over an empty or truncated measurement passes trivially. Pin the shape: all
    // three wasm modules, the worker bundle, the control file, and a page.
    const keys = Object.keys(groups);
    for (const required of [
      "engines/pdfium.wasm",
      "engines/qpdf.wasm",
      "engines/burrow_wasm_bg.wasm",
      "engines/burrow-worker.js",
      "engines/control.txt",
      "page",
    ]) {
      expect(keys, `${required} is not in the measured payload`).toContain(required);
    }
    // PDFium is 82% of it (spike 0001). If it ever is not, either the payload is wrong or
    // something very large arrived.
    expect(groups["engines/pdfium.wasm"].brotli / measurement.total.brotli).toBeGreaterThan(0.7);
  });

  it("catches a regression split across three files, which no per-file budget would", () => {
    // The reason the total exists, planted rather than argued. Each artifact grows by 4% --
    // comfortably inside its own 10% line -- and the total must still fail.
    //
    // Computed against the *recorded* measurements rather than the live ones, so this test
    // asserts a property of the budget file and cannot be made vacuous by the build changing.
    const grown = Object.fromEntries(
      Object.entries(budget.artifacts).map(([k, v]) => [k, Math.round(v.measured_brotli * 1.04)]),
    );

    for (const [key, value] of Object.entries(grown)) {
      expect(
        value,
        `${key} at +4% would already exceed its own budget, so this test proves nothing`,
      ).toBeLessThanOrEqual(budget.artifacts[key].budget_brotli);
    }

    const total = Object.values(grown).reduce((n, v) => n + v, 0);
    expect(
      total,
      "a 4% regression spread across every artifact would pass the total budget. " +
        "The total's headroom is too loose to be the gate it claims to be",
    ).toBeGreaterThan(budget.total.budget_brotli);
  });

  it("has per-artifact measurements that add up to the recorded total", () => {
    // The "+4% across three files" test above sums `artifacts[*].measured_brotli` and
    // compares the result to `total.budget_brotli`. That is only meaningful while the two
    // halves of this file describe the same build: a per-artifact number left stale after a
    // re-measure would silently weaken it, and nothing else would notice.
    const summed = Object.values(budget.artifacts).reduce((n, a) => n + a.measured_brotli, 0);
    expect(
      summed,
      "size-budget.json's per-artifact measurements do not sum to its recorded total; " +
        "one half was re-measured and the other was not",
    ).toBe(budget.total.measured_brotli);
  });

  it("records the measurement each budget was set from", () => {
    // `measured_brotli` is what makes a budget auditable: a reviewer can see how much slack a
    // line has without rebuilding. A budget below its own measurement is a typo that would
    // fail on the very build it was taken from.
    for (const [key, line] of Object.entries(budget.artifacts)) {
      expect(line.measured_brotli, `${key} records no measurement`).toBeGreaterThan(0);
      expect(line.budget_brotli, `${key}'s budget is below its own measurement`).toBeGreaterThan(
        line.measured_brotli,
      );
      expect(line.why, `${key} has no stated reason`).toBeTruthy();
    }
    expect(budget.total.budget_brotli).toBeGreaterThan(budget.total.measured_brotli);
  });

  it("has not drifted far from the recorded measurements without anyone noticing", () => {
    // A budget file whose measurements are stale still gates correctly, but it stops being
    // readable: `measured_brotli` no longer tells a reviewer how much slack there is. This
    // fails at half the remaining headroom, which is a request to re-measure rather than a
    // regression -- and it fires before the budget does, so it is a warning with a diff
    // rather than a red build with no explanation.
    const halfway =
      budget.total.measured_brotli +
      (budget.total.budget_brotli - budget.total.measured_brotli) / 2;
    expect(
      measurement.total.brotli,
      `first load is ${kb(measurement.total.brotli)}, more than halfway from the recorded ` +
        `${kb(budget.total.measured_brotli)} to the ${kb(budget.total.budget_brotli)} budget. ` +
        `Re-measure and update size-budget.json, or find out what grew`,
    ).toBeLessThanOrEqual(halfway);
  });
});
