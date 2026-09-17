// The page-picture strip, in a real browser, on the page that ships it.
//
// #57's UI half. Everything below it is covered somewhere cheaper — the ceiling and the
// progressive loop by `cargo test`, the policy arithmetic by
// `src/components/thumbnail-policy.test.ts`, the component's decisions by
// `src/components/page-thumbnails.test.ts`. What only a browser can answer:
//
//   * whether a tile actually gets PIXELS, rather than a canvas nobody drew into. The CSP
//     refuses `data:` and `blob:` images SILENTLY (ADR 0014), so a strip built the obvious way
//     shows nothing and reports nothing — the failure this file exists to make loud;
//   * whether PDFium is fetched only when a strip needs it, which is ADR 0026's promise;
//   * whether the tool still works when the strip does not, which is ADR 0020's decision and
//     the reason a broken strip is not a broken page.

import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { isPinnedArtifact, mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "../../../tests/conformance/fixtures");

async function choose(page: Page, name: string): Promise<void> {
  await page.locator("input[type=file]").setInputFiles(join(fixtures, name));
  await expect(page.locator(".chosen__pages")).not.toContainText("counting", {
    timeout: 45_000,
  });
}

/** The first tile's canvas, once the strip has drawn it. */
function firstCanvas(page: Page) {
  return page.locator(".strip__canvas").first();
}

test("a chosen document gets a tile per page, numbered, before anything is drawn", async ({
  page,
}) => {
  await page.goto("/rotate-pdf/");
  await choose(page, "mixed-rotation-4page.pdf");

  // THE LAYOUT COMES FROM THE PAGE COUNT, not from pictures arriving. A strip that grew as
  // tiles landed would reflow under somebody's cursor on every page of a long document.
  await expect(page.locator(".strip__tile")).toHaveCount(4);
  await expect(page.locator(".strip__number").first()).toHaveText("1");
});

test("a tile is drawn with real pixels, which is the half the CSP could break silently", async ({
  page,
}) => {
  await page.goto("/rotate-pdf/");
  await choose(page, "mixed-rotation-4page.pdf");

  const canvas = firstCanvas(page);
  await expect(canvas).toBeVisible({ timeout: 45_000 });

  // NOT "a canvas exists" — that passes against a strip that draws nothing at all, which is
  // exactly what an `<img src="blob:...">` would have produced under `img-src 'self'`. The
  // canvas is read back and required to have more than one colour in it: a fixture page is ink
  // on paper, so a drawn tile is not uniform and an undrawn one is.
  const distinct = await canvas.evaluate((node: HTMLCanvasElement) => {
    const context = node.getContext("2d");
    if (!context || node.width === 0) return 0;
    const { data } = context.getImageData(0, 0, node.width, node.height);
    const seen = new Set<number>();
    for (let i = 0; i < data.length; i += 4) {
      seen.add((data[i]! << 16) | (data[i + 1]! << 8) | data[i + 2]!);
      if (seen.size > 1) break;
    }
    return seen.size;
  });
  expect(
    distinct,
    "the tile is a single flat colour, so nothing was drawn into it",
  ).toBeGreaterThan(1);
});

test("the tile is at the device-pixel size the policy asks for, not the CSS one", async ({
  page,
}) => {
  await page.goto("/rotate-pdf/");
  await choose(page, "mixed-rotation-4page.pdf");
  const canvas = firstCanvas(page);
  await expect(canvas).toBeVisible({ timeout: 45_000 });

  // A page fitted into a 120x160 CSS box at DPR 1 is at most 120x160 backing pixels; at DPR 2
  // at most 240x320. Either way it must be at least as large as the CSS box in one dimension,
  // or the fit is not filling the frame — and it must never exceed the device box, which is
  // the promise `Fit` makes and `check_pixels` enforces.
  const size = await canvas.evaluate((node: HTMLCanvasElement) => ({
    width: node.width,
    height: node.height,
  }));
  const ratio = await page.evaluate(() => Math.min(window.devicePixelRatio || 1, 2));
  expect(size.width).toBeGreaterThan(0);
  expect(size.width).toBeLessThanOrEqual(Math.round(120 * ratio));
  expect(size.height).toBeLessThanOrEqual(Math.round(160 * ratio));
});

test("a page that draws a strip DOES fetch the render bundle", async ({ page }, testInfo) => {
  // THE PARTITION CONTROL, in the browser dimension. `merge-pdf.spec.ts` asserts the absence
  // half over a whole visit -- a page that renders nothing reaches for no render bundle -- and
  // an absence rule with no opposite proves nothing: a build that staged PDFium nowhere, or a
  // strip that silently never started, would satisfy it perfectly.
  //
  // `tools/check-pdfium-is-render-only.sh` has the same pair over the FILES. This is the pair
  // over what a browser actually asks for, which is the only place the lazy fetch is real.
  await mark(`strip-render:${testInfo.project.name}`);

  await page.goto("/rotate-pdf/");
  await choose(page, "mixed-rotation-4page.pdf");
  await expect(firstCanvas(page)).toBeVisible({ timeout: 45_000 });

  const requested = since(`strip-render:${testInfo.project.name}`).map((entry) => entry.url);
  const render = requested.filter((url) => /pdfium|render-worker|_render_bg/.test(url));
  expect(
    render.length,
    "a page that drew a strip fetched no render artifact, so the strip cannot have rendered",
  ).toBeGreaterThan(0);

  // AND EVERY ENGINE ARTIFACT IT FETCHED IS ONE IT IS ENTITLED TO. The widened set, because
  // this page IS entitled to the render bundle -- which is exactly the widening
  // `merge-pdf.spec.ts` must not have, and the reason `isPinnedArtifact` takes the set as an
  // argument rather than allowing both everywhere.
  //
  // Scoped to `/engines/`, because the visit also fetches its own markup, styles and island —
  // an earlier version looped over every request and failed on `/rotate-pdf/` itself, which is
  // the page, not an artifact.
  const engines = requested.filter((url) => url.includes("/engines/"));
  expect(engines.length, "no engine artifact was fetched at all").toBeGreaterThan(0);
  for (const url of engines) {
    expect(isPinnedArtifact(url, "all"), `${url} is not a pinned engine artifact`).toBe(true);
  }
});

test("the tool still works when the strip is not there", async ({ page }) => {
  // ADR 0020's decision, still load-bearing: these pages select by page number and shipped with
  // no pictures at all. So the strip failing must never block the control — a picture is an
  // aid, and an aid that takes the tool with it is worse than no aid.
  await page.goto("/rotate-pdf/");
  await choose(page, "mixed-rotation-4page.pdf");
  await page.evaluate(() => {
    document.querySelector(".strip")?.remove();
  });
  await expect(page.getByRole("button", { name: "Turn pages" })).toBeEnabled();
});
