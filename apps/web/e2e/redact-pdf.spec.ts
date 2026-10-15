// The held /redact-pdf page's island, driven as a person drives it (#136, S3).
//
// HARNESS BUILD ONLY: the route is held, so it exists only where `BURROW_HARNESS=1` built it,
// which is what the e2e global setup builds. A production build has no such page;
// `src/production-build.test.ts` asserts that.
//
// What this holds: a file is read for its pages' frames through redaction's own worker; a region
// typed in points is checked against the page before anything is sent; a run hands back a new
// file through a link the person activates; and a refusal is said by its category, with its rule
// id, never with the engine's message.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { CATEGORIES } from "../src/components/redact-messages";
import { isBrowserPolicyReport } from "./console-noise";
import { isRedactionArtifact, mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "../../../tests/redaction/fixtures");
const conformance = resolve(here, "../../../tests/conformance/fixtures");
const generated = resolve(here, "../../../tests/redaction/generated");

/** A committed fixture whose whole first page redacts (`outcomes.tsv`: `whole OK`). */
const REDACTS = "producer-writer.pdf";
/** A committed fixture whose whole first page reaches a picture (`[image-in-region]`). */
const PICTURE = "producer-ocr-scan.pdf";

async function open(
  page: Page,
  name: string,
  dir: string = fixtures,
): Promise<{ width: number; height: number }> {
  await page.goto("/redact-pdf/");
  await page.getByTestId("redact-file").setInputFiles(join(dir, name));
  const frame = page.getByTestId("redact-frame");
  await expect(frame).toBeVisible({ timeout: 45_000 });
  return {
    width: Number(await frame.getAttribute("data-width")),
    height: Number(await frame.getAttribute("data-height")),
  };
}

async function area(page: Page, left: string, top: string, width: string, height: string) {
  await page.getByTestId("redact-left").fill(left);
  await page.getByTestId("redact-top").fill(top);
  await page.getByTestId("redact-width").fill(width);
  await page.getByTestId("redact-height").fill(height);
}

test("a whole page redacts, and the new file is handed over only when asked for", async ({
  page,
}) => {
  const frame = await open(page, REDACTS);
  await expect(page.locator(".chosen__pages")).toContainText("page");
  await area(page, "0", "0", String(frame.width), String(frame.height));
  await page.getByTestId("redact-run").click();
  const link = page.getByTestId("redact-download");
  await expect(link).toBeVisible({ timeout: 45_000 });
  await expect(link).toHaveText(/producer-writer-redacted\.pdf/);

  const download = page.waitForEvent("download");
  await link.click();
  const stream = await (await download).createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  const out = Buffer.concat(chunks);
  expect(out.subarray(0, 5).toString("latin1")).toBe("%PDF-");
  expect(out.equals(readFileSync(join(fixtures, REDACTS))), "the output is the input").toBe(false);

  // A changed request hides the old result's link rather than leaving it labelled for it.
  await page.getByTestId("redact-width").fill("10");
  await expect(link).toBeHidden();
});

test("a refusal is said by its category, with its rule id", async ({ page }) => {
  const frame = await open(page, PICTURE);
  await area(page, "0", "0", String(frame.width), String(frame.height));
  await page.getByTestId("redact-run").click();
  const notice = page.getByTestId("redact-notice");
  await expect(notice).toBeVisible({ timeout: 45_000 });
  await expect(notice).toContainText(CATEGORIES.pictures.title);
  await expect(notice).toContainText("(image-in-region)");
  // THE ENGINE'S OWN SENTENCE NEVER REACHES THE PAGE (ADR 0009).
  await expect(notice).not.toContainText("which the redaction removes no part of");
  await expect(page.getByTestId("redact-download")).toHaveCount(0);
});

test("a region past the page's edge is refused before anything is sent", async ({ page }) => {
  const frame = await open(page, REDACTS);
  await area(page, "0", "0", String(frame.width + 1), "10");
  await expect(page.locator("#region-problem")).toContainText("runs past the edge of the page");
  await expect(page.getByTestId("redact-run")).toBeDisabled();

  await area(page, "0", "0", "1e3", "10");
  await expect(page.locator("#region-problem")).toContainText("Width needs a number of points");
  await expect(page.getByTestId("redact-run")).toBeDisabled();
});

test("a page number outside the document is refused beside its box", async ({ page }) => {
  await open(page, REDACTS);
  await page.getByTestId("redact-page").fill("999");
  await expect(page.locator("#page-problem")).toContainText("Type a page number from 1 to");
  await expect(page.getByTestId("redact-run")).toBeDisabled();
});

test("a page a frame rule refuses says so before anything is typed", async ({ page }) => {
  // `page-frames.tsv`: every page of this one is refused `page-attribute-inherited`.
  await page.goto("/redact-pdf/");
  await page
    .getByTestId("redact-file")
    .setInputFiles(join(conformance, "inherited-rotation-6page.pdf"));
  const refused = page.getByTestId("redact-page-refused");
  await expect(refused).toBeVisible({ timeout: 45_000 });
  await expect(refused).toContainText(CATEGORIES["page-shape"].title);
  await expect(refused).toContainText("(page-attribute-inherited)");
  await expect(page.getByTestId("redact-left")).toHaveCount(0);
  await expect(page.getByTestId("redact-run")).toBeDisabled();
});

test("Stop ends a run without a result or a refusal", async ({ page }) => {
  const frame = await open(page, REDACTS);
  await area(page, "0", "0", String(frame.width), String(frame.height));
  // RUN AND STOP IN ONE TASK OF THE PAGE, so the stop cannot lose a race with the run: the run
  // needs a fresh worker (redaction's recycles after every reply), and nothing it does can answer
  // before the next macrotask. A Playwright click between the two raced, and lost about once in 24.
  const stopped = await page.evaluate(async () => {
    (document.querySelector('[data-testid="redact-run"]') as HTMLButtonElement).click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const stop = document.querySelector<HTMLButtonElement>('[data-testid="redact-stop"]');
    stop?.click();
    return stop !== null;
  });
  expect(stopped, "the Stop button was not there to press while the run was working").toBe(true);
  await expect(page.getByTestId("redact-stop")).toHaveCount(0);
  await expect(page.getByTestId("redact-download")).toHaveCount(0);
  await expect(page.getByTestId("redact-notice")).toHaveCount(0);
  await expect(page.getByTestId("redact-run")).toBeEnabled();
  // And a run after it still works.
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
});

test("the two §7 notes appear beside the result exactly when the reply counted them", async ({
  page,
}) => {
  // GENERATED FIXTURES, which the `web` job receives as the `test` job's artifact. Measured
  // natively (2026-10-15): the first retains one font another page uses, and the second drops one
  // passage's carried text; no committed fixture does either.
  const retained = await open(page, "nearmiss-oc-on-another-page.pdf", generated);
  await area(page, "0", "0", String(retained.width), String(retained.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  const notes = page.getByTestId("redact-result-note");
  await expect(notes).toHaveCount(1);
  await expect(notes).toContainText("One font on this page is also used on other pages");

  const dropped = await open(page, "09-actualtext.pdf", generated);
  await area(page, "0", "0", String(dropped.width), String(dropped.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await expect(notes).toHaveCount(1);
  await expect(notes).toContainText("had alternative text, and all of it is gone");

  // THE NEAR-MISS: a run that counted neither shows neither.
  const plain = await open(page, REDACTS);
  await area(page, "0", "0", String(plain.width), String(plain.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await expect(notes).toHaveCount(0);
});

test("the page's console stays empty through a run and a refusal", async ({ page }) => {
  const noise: string[] = [];
  page.on("console", (message) => noise.push(`page console.${message.type()}: ${message.text()}`));
  page.on("pageerror", (error) => noise.push(`pageerror: ${error.message}`));

  const frame = await open(page, REDACTS);
  await area(page, "0", "0", String(frame.width), String(frame.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });

  await page.getByTestId("redact-file").setInputFiles(join(fixtures, PICTURE));
  const picture = page.getByTestId("redact-frame");
  await expect(picture).toBeVisible({ timeout: 45_000 });
  await area(
    page,
    "0",
    "0",
    String(await picture.getAttribute("data-width")),
    String(await picture.getAttribute("data-height")),
  );
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-notice")).toBeVisible({ timeout: 45_000 });

  const ours = noise.filter((line) => !isBrowserPolicyReport(line));
  expect(ours, `the page must add nothing to the console:\n${ours.join("\n")}`).toEqual([]);
});

test("redacting makes no request beyond redaction's pinned engine artifacts", async ({
  page,
}, testInfo) => {
  // THE MARKER GOES BEFORE `goto`, so start-up is in the window too: the page must never reach
  // for the base worker or the render bundle, and a respawn re-fetches only redaction's set.
  await mark(`redact-page:${testInfo.project.name}`);
  const frame = await open(page, REDACTS);
  await area(page, "0", "0", String(frame.width), String(frame.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });

  const after = since(`redact-page:${testInfo.project.name}`);
  const engines = after.filter((entry) => entry.url.startsWith("/engines/"));
  const unexpected = engines.filter((entry) => !isRedactionArtifact(entry.url));
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the redaction page fetched an engine artifact it does not use",
  ).toEqual([]);
  // THE CONTROL: redaction's own module was fetched, so the window watched something.
  expect(
    engines.filter((entry) => /burrow_wasm_redact_bg/.test(entry.url)).length,
    "redaction's module was not fetched, so this test watched nothing",
  ).toBeGreaterThan(0);
  // AND NOTHING BUT GETs of the page's own static files: no upload of any kind.
  const posted = after.filter((entry) => entry.method !== "GET");
  expect(posted.map((entry) => `${entry.method} ${entry.url}`)).toEqual([]);
});
