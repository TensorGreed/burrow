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

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "../../../tests/redaction/fixtures");

/** A committed fixture whose whole first page redacts (`outcomes.tsv`: `whole OK`). */
const REDACTS = "producer-writer.pdf";
/** A committed fixture whose whole first page reaches a picture (`[image-in-region]`). */
const PICTURE = "producer-ocr-scan.pdf";

async function open(page: Page, name: string): Promise<{ width: number; height: number }> {
  await page.goto("/redact-pdf/");
  await page.getByTestId("redact-file").setInputFiles(join(fixtures, name));
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
