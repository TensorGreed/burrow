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

import { expect, test, type Page, type Worker } from "@playwright/test";

import { isPinnedArtifact, mark, since } from "./request-log";

import { STRIP_WITHDRAWN } from "../src/components/strip-copy.js";
import { END_DEADLINE_MS } from "../src/components/strip-lifecycle.js";

// SKIPPED WHENEVER THE STRIP IS WITHDRAWN, AND DERIVED FROM THE FLAG RATHER THAN COMMENTED OUT.
//
// The strip was withdrawn from 2026-09-17 to 2026-09-26 (#107) and is mounted again. While
// `STRIP_WITHDRAWN` is true, `/split-pdf` and `/rotate-pdf` do not mount it, so every test
// below would fail on an element that is deliberately absent. Deleting them, or marking
// them `test.skip` by hand, would mean restoring the strip depends on somebody remembering
// that these exist -- the same trap `strip-copy.ts` exists to close for the prose.
//
// The component, its unit tests and the render worker are untouched; `measure.spec.ts` still
// drives the renderer through the harness, so the ENGINE path stays covered while the UI is
// withdrawn. Flip the flag and these run again, unchanged. #107.
test.skip(STRIP_WITHDRAWN, "the page-picture strip is withdrawn (#107)");

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
  // THE CANVAS WAS ASKED FOR AS A CPU ONE (#107), read from the browser rather than from the
  // source: the context the strip created is the one returned here, and its settings say what
  // it was created with. That the setting yields an unaccelerated buffer is WebKit's
  // `allocateImageBuffer` plus #107's 0-in-1,160 measurement, not something this reads.
  // Where `getContextAttributes` is missing the answer is "unknown", which fails, rather than
  // a pass that measured nothing.
  const willReadFrequently = await canvas.evaluate(
    (node: HTMLCanvasElement) =>
      node.getContext("2d")?.getContextAttributes?.().willReadFrequently ?? "unknown",
  );
  expect(willReadFrequently, "the strip's canvas is GPU-backed, which reopens #107").toBe(true);

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

// ---- #199 for the render worker (D1, D4): one document per worker, pixels zeroed --------------

/**
 * Hold the render worker's PDFium download, so the strip's first request is known to be in flight
 * until `release` is called. Later fetches go through.
 */
function holdFirstPdfium(page: Page): { arrived: Promise<void>; release: () => Promise<void> } {
  let held: (() => Promise<void>) | null = null;
  const arrived = new Promise<void>((resolve) => {
    void page.route(/\/pdfium\.[0-9a-f]+\.wasm$/, (route) => {
      if (held !== null) return route.continue();
      held = () => route.continue();
      resolve();
    });
  });
  return { arrived, release: () => held!() };
}

/** Count every `putImageData`, so what was drawn before a worker closed can be read back. */
async function countPaints(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const original = CanvasRenderingContext2D.prototype.putImageData;
    const scope = globalThis as unknown as { __paints: number };
    scope.__paints = 0;
    CanvasRenderingContext2D.prototype.putImageData = function (
      this: CanvasRenderingContext2D,
      ...args: unknown[]
    ) {
      scope.__paints += 1;
      return (original as (...a: unknown[]) => void).apply(this, args);
    } as typeof original;
  });
}

/**
 * The paint count when `worker`'s `close` reaches the test: before a new worker could have
 * compiled PDFium and drawn (about 190 ms measured, S5).
 */
function paintsWhenItCloses(
  page: Page,
  worker: Worker,
): { paints: Promise<number>; closedYet: () => boolean } {
  let closed = false;
  const paints = new Promise<number>((resolve) =>
    worker.on("close", () => {
      closed = true;
      void page
        .evaluate(() => (globalThis as unknown as { __paints: number }).__paints)
        .then(resolve);
    }),
  );
  return { paints, closedYet: () => closed };
}

/**
 * The page's render worker, found by what its bundle carries: only the render bundle defines the
 * PDFium bridge. Polled, because the strip builds its worker on first use.
 */
async function renderWorker(page: Page): Promise<Worker> {
  const deadline = Date.now() + 45_000;
  while (Date.now() < deadline) {
    for (const worker of page.workers()) {
      const isRender = await worker
        .evaluate(
          () =>
            typeof (self as unknown as Record<string, unknown>)["__burrow_pdfium_copy_out"] ===
            "function",
        )
        .catch(() => false);
      if (isRender) return worker;
    }
    await page.waitForTimeout(100);
  }
  throw new Error("no render worker appeared");
}

/** Whether a canvas holds any pixel that is not fully transparent. */
async function inked(page: Page): Promise<boolean> {
  return firstCanvas(page).evaluate((node: HTMLCanvasElement) => {
    const context = node.getContext("2d", { willReadFrequently: true });
    if (!context || node.width === 0 || node.height === 0) return false;
    const data = context.getImageData(0, 0, node.width, node.height).data;
    for (let i = 3; i < data.length; i += 4) if (data[i] !== 0) return true;
    return false;
  });
}

test("a render worker draws one document: choosing another ends it (#199)", async ({ page }) => {
  await page.goto("/rotate-pdf/");
  await choose(page, "pages-10.pdf");
  await expect(firstCanvas(page)).toBeVisible({ timeout: 45_000 });
  await expect.poll(() => inked(page), { timeout: 45_000 }).toBe(true);
  const first = await renderWorker(page);
  const closed = new Promise<void>((resolve) => first.on("close", () => resolve()));

  await choose(page, "mixed-rotation-4page.pdf");
  await closed;
  // AND THE NEW DOCUMENT IS DRAWN, on a worker of its own.
  await expect.poll(() => inked(page), { timeout: 45_000 }).toBe(true);
  const second = await renderWorker(page);
  expect(second, "the second document was drawn by the first document's worker").not.toBe(first);
});

test("leaving the page ends the render worker and clears every tile's pixels (#199)", async ({
  page,
}) => {
  await page.goto("/rotate-pdf/");
  await choose(page, "pages-10.pdf");
  await expect.poll(() => inked(page), { timeout: 45_000 }).toBe(true);
  const worker = await renderWorker(page);
  const closed = new Promise<void>((resolve) => worker.on("close", () => resolve()));
  // HELD ACROSS THE UNMOUNT: the canvas the tile drew into, so what it keeps after leaving the DOM
  // can be read.
  const canvas = await firstCanvas(page).elementHandle();
  expect(canvas).not.toBeNull();

  // `pagehide` is what a navigation or a back-forward cache entry fires.
  await page.evaluate(() => globalThis.dispatchEvent(new PageTransitionEvent("pagehide")));
  await closed;
  await expect(page.locator(".strip__canvas")).toHaveCount(0);
  const kept = await canvas!.evaluate((node: HTMLCanvasElement) => ({
    width: node.width,
    height: node.height,
    connected: node.isConnected,
  }));
  expect(kept, "the canvas left the DOM still holding its picture").toEqual({
    width: 0,
    height: 0,
    connected: false,
  });
});

test("an operation ends the render worker before it runs (#199)", async ({ page }) => {
  await page.goto("/rotate-pdf/");
  await choose(page, "pages-10.pdf");
  await expect.poll(() => inked(page), { timeout: 45_000 }).toBe(true);
  const worker = await renderWorker(page);
  const closed = new Promise<void>((resolve) => worker.on("close", () => resolve()));

  await page.getByRole("button", { name: "Turn pages" }).click();
  await closed;
  await expect(page.locator(".result a.download")).toBeVisible({ timeout: 45_000 });
});

test("a document chosen while a render is in flight is drawn once it settles, and the old is not (#199)", async ({
  page,
}) => {
  // A RENDER KNOWN TO BE IN FLIGHT, held by the harness rather than hoped for: the render worker's
  // PDFium download is held, so the strip's first request for `pages-10.pdf` cannot settle until
  // the test lets it. The document is changed inside that window, which is the path the strip's
  // `finally` exists for: the end waits for the render to settle (`ending`, never mid-render,
  // #361), the old pages are discarded as stale, and `pump` is asked again for the new document's
  // tiles -- nothing else will ask, since their intersection already found `running` set.
  const { arrived, release } = holdFirstPdfium(page);
  await countPaints(page);

  await page.goto("/rotate-pdf/");
  await choose(page, "pages-10.pdf");
  await arrived;
  const first = await renderWorker(page);
  const ended = paintsWhenItCloses(page, first);

  const changed = Date.now();
  await choose(page, "mixed-rotation-4page.pdf");
  // The new document's tiles, not yet drawn: the held render is still in flight.
  await expect(page.locator(".strip__tile")).toHaveCount(4);
  await expect(page.locator(".strip__canvas")).toHaveCount(0);
  // RELEASED SHORT OF `END_DEADLINE_MS`, with the old worker still alive: so the end was not
  // taken while the render was held. Nothing here times the close itself; what this test
  // distinguishes is that no old page is painted and that the new document is drawn by a worker
  // of its own (the re-pump), below.
  await page.waitForTimeout(300);
  expect(ended.closedYet(), "the old worker ended before its render settled").toBe(false);
  expect(Date.now() - changed, "the hold ran into the end deadline").toBeLessThan(END_DEADLINE_MS);
  await release();

  // THE OLD WORKER CLOSES, once its render settled. AND NO OLD TILE SHOWED: every picture the
  // old worker produced arrived for a document nobody shows any more.
  const paintsAtClose = await ended.paints;
  expect(paintsAtClose, "a page of the previous document was painted").toBe(0);
  // THE NEW DOCUMENT IS DRAWN, on a worker of its own: the re-pump.
  await expect.poll(() => inked(page), { timeout: 45_000 }).toBe(true);
  const second = await renderWorker(page);
  expect(second, "the second document was drawn by the first document's worker").not.toBe(first);
});

test("a render in flight across a back-forward cache hide and restore is never painted (#199)", async ({
  page,
}) => {
  // OWNER, 2026-10-16, (a): a worker frozen with the page is accepted, and what it posts is
  // dropped -- zeroed, never painted. The render is held across `pagehide` and a persisted
  // `pageshow`, then released: every page it delivers arrives for a generation nobody shows.
  const { arrived, release } = holdFirstPdfium(page);
  await countPaints(page);

  await page.goto("/rotate-pdf/");
  await choose(page, "pages-10.pdf");
  await arrived;
  const first = await renderWorker(page);
  const ended = paintsWhenItCloses(page, first);

  await page.evaluate(() => {
    globalThis.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true }));
    globalThis.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
  });
  expect(ended.closedYet(), "the worker was terminated mid-render (#361)").toBe(false);
  await release();

  expect(await ended.paints, "a page the frozen render produced was painted").toBe(0);
  // AND THE RESTORED PAGE DRAWS, on a worker of its own.
  await expect.poll(() => inked(page), { timeout: 45_000 }).toBe(true);
  const second = await renderWorker(page);
  expect(second, "the restored page drew on the frozen page's worker").not.toBe(first);
});
