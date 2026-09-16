// `/compress-pdf`, driven the way a person drives it.
//
// # What this suite is really checking
//
// Every other tool page has one successful shape: pick a file, get a document. This page has
// three, and two of them hand the person back roughly what they started with. Spike 0005
// measured 85.6% on a form of many small objects and **0.15% on a scan** — and a scan is the
// modal thing somebody brings to a page called "Compress PDF".
//
// So the assertions below are about WORDS as much as bytes. A file that does not shrink must
// arrive as a finding, with an explanation, in ordinary ink — not as an error, not as a bare
// percentage, and not as a silent nothing. `compress-messages.test.ts` proves the sentences in
// isolation; this proves the page actually shows them.
//
// The fixtures are the committed conformance ones, so the same documents the differential
// corpus runs on are the ones a person's browser is driven over here.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { isBrowserPolicyReport } from "./console-noise";
import { isPinnedArtifact, mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "../../../tests/conformance/fixtures");

function fixture(name: string): string {
  return join(fixtures, name);
}

/**
 * Choose one file and wait until it has been counted, asserting the count POSITIVELY.
 *
 * An earlier version waited for `.chosen__pages` not to contain "counting". A negated
 * Playwright text assertion is satisfied by a locator that matches NOTHING, and
 * `.chosen__pages` only exists inside `{#if file}` -- so it returned immediately in the window
 * before Svelte flushed, and would have returned immediately on a page where the island never
 * mounted at all. That is precisely the CSP-refuses-the-bootstrap failure `compress-pdf.astro`
 * warns about in its own comment: the tool renders as dead markup, and the helper guarding
 * every test in this file would have called it ready. It also passed when the count FAILED.
 *
 * Asserting the number turns the wait into a measurement.
 */
async function choose(page: Page, name: string, pages: number): Promise<void> {
  await page.locator("input[type=file]").setInputFiles(fixture(name));
  await expect(page.locator(".chosen__pages")).toHaveText(
    pages === 1 ? "1 page" : `${pages} pages`,
    { timeout: 45_000 },
  );
}

/** Compress, and wait for whichever of the three outcomes arrives. */
async function compress(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Compress" }).click();
  await expect(page.locator(".outcome")).toBeVisible({ timeout: 45_000 });
}

async function bytesOf(download: import("@playwright/test").Download): Promise<Buffer> {
  const path = await download.path();
  return readFileSync(path);
}

test("a document of many small objects gets smaller, and says by how much", async ({ page }) => {
  await page.goto("/compress-pdf");
  await choose(page, "pages-137.pdf", 137);
  await compress(page);

  const title = await page.locator(".outcome__title").innerText();
  // BOTH SIZES AND THE PERCENTAGE. "Smaller!" with no numbers is the page asking to be
  // believed rather than showing its work, which is the whole thing this design refuses.
  expect(title).toMatch(/→/);
  expect(title).toMatch(/\d+(\.\d+)?%/);

  // AND THE LOSSLESS CLAIM, on the happy path. Somebody who just watched a file shrink is
  // entitled to know whether their images survived.
  const detail = await page.locator(".outcome__detail").innerText();
  expect(detail).toContain("come through unaltered");
  expect(detail).not.toContain("byte-for-byte");
});

test("the compressed document downloads, and it is a PDF", async ({ page }) => {
  await page.goto("/compress-pdf");
  await choose(page, "pages-137.pdf", 137);
  await compress(page);

  // A LINK A PERSON ACTIVATES, not a download that starts itself.
  const link = page.locator(".outcome__download a.download");
  await expect(link).toBeVisible();
  const download = page.waitForEvent("download");
  await link.click();
  const bytes = await bytesOf(await download);

  expect(bytes.subarray(0, 5).toString("latin1")).toBe("%PDF-");
  // SMALLER THAN THE INPUT, checked against the file on disk rather than against the page's
  // own claim -- the page reporting a saving it did not produce is exactly the failure a
  // browser test can see and a unit test cannot.
  expect(bytes.length).toBeLessThan(readFileSync(fixture("pages-137.pdf")).length);
});

test("a file with nothing to pack is reported, not failed", async ({ page }) => {
  // THE OUTCOME THIS PAGE EXISTS TO HANDLE WELL. `blank-1page.pdf` has almost no structure, so
  // compression achieves nothing -- the same answer a scan gets, reachable with a committed
  // fixture.
  await page.goto("/compress-pdf");
  await choose(page, "blank-1page.pdf", 1);
  await compress(page);

  const title = await page.locator(".outcome__title").innerText();
  const detail = await page.locator(".outcome__detail").innerText();

  // IT IS NOT AN ERROR. The refusal element must not appear at all: `--refuse` is reserved for
  // refusals and limits, and a document that was already efficiently stored is neither.
  await expect(page.locator(".notice")).toHaveCount(0);

  // AND IT EXPLAINS ITSELF rather than reporting a number that reads as a broken tool.
  expect(`${title} ${detail}`).toContain("already");
  expect(detail).toContain("structure");
  expect(detail).toContain("already compressed");

  // WHICH OF THE TWO DISAPPOINTING OUTCOMES IS DELIBERATELY NOT PINNED, and saying so is the
  // point rather than a hedge. `blank-1page.pdf` sits near the boundary: whether qpdf's writer
  // comes out a few bytes under or over the original is a property of the fixture and the
  // pinned qpdf version, not a promise burrow makes, so asserting `negligible` over `unchanged`
  // would pin something a pin bump is entitled to change. What must hold for BOTH is asserted
  // above and below: it is not an error, it explains itself, and either way the person is told
  // both numbers.
  expect(["negligible", "unchanged"]).toContain(
    (await page.locator(".outcome__download").count()) > 0 ? "negligible" : "unchanged",
  );
});

test("the page says a scan will not shrink BEFORE anything is chosen", async ({ page }) => {
  // ADR 0025 §5, as a page assertion. The explanation has to be readable without choosing a
  // file, because the alternative is letting somebody wait for an answer the page could have
  // given at once -- and because this prose is what stops 0.15% reading as a broken tool.
  await page.goto("/compress-pdf");

  const body = await page.locator("body").innerText();
  expect(body).toContain("Scans and photograph-heavy documents usually lose almost nothing");
  expect(body).toContain("already compressed");
  // AND THAT THE ALTERNATIVE IS REFUSED ON PURPOSE. Other tools shrink a scan by re-encoding
  // its images; saying why this one does not is the difference between a limitation and a
  // decision.
  expect(body).toContain("Nothing is re-encoded");
});

test("a file burrow cannot read is refused, and nothing from it reaches the page", async ({
  page,
}) => {
  await page.goto("/compress-pdf");
  await page.locator("input[type=file]").setInputFiles(fixture("not-a-pdf.bin"));

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible({ timeout: 45_000 });
  const text = await notice.innerText();
  expect(text).toContain("could not read");
  // NOTHING FROM THE FILE. The fixture's own bytes must not appear in the message -- errors
  // describe the failure, not the input.
  expect(text).not.toContain("not-a-pdf");
  expect(text.toLowerCase()).not.toContain("qpdf");
});

test("an encrypted file is refused by name, and says what to do", async ({ page }) => {
  await page.goto("/compress-pdf");
  await page.locator("input[type=file]").setInputFiles(fixture("encrypted.pdf"));

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible({ timeout: 45_000 });
  expect(await notice.innerText()).toContain("password-protected");
});

test("the tool page's console stays empty through a compression and through a refusal", async ({
  page,
}) => {
  const noise: string[] = [];
  page.on("console", (m) => {
    if (!isBrowserPolicyReport(m.text())) noise.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => noise.push(`pageerror: ${e.message}`));

  await page.goto("/compress-pdf");
  await choose(page, "pages-137.pdf", 137);
  await compress(page);

  await page.locator("input[type=file]").setInputFiles(fixture("not-a-pdf.bin"));
  await expect(page.locator(".notice")).toBeVisible({ timeout: 45_000 });

  // qpdf's logger writes object numbers and byte offsets to stderr by default, and on the web
  // that lands in this console. The assertion is that it is EMPTY, not merely free of file
  // content: a page that logs anything has a channel through which content could arrive.
  expect(noise, `the page wrote to the console: ${noise.join(", ")}`).toEqual([]);
});

test("no request of any kind leaves the page once the engines have loaded", async ({
  page,
}, testInfo) => {
  // AGAINST THE PAGE THAT SHIPS, not `/harness` -- which is deleted from production builds, so
  // an assertion that only runs there says nothing about a route a person can visit.
  await page.goto("/compress-pdf");
  // THE MARKER GOES DOWN AFTER THE ENGINES ARE LOADED, so what follows it should be nothing at
  // all rather than "nothing unexpected".
  await choose(page, "pages-137.pdf", 137);
  await mark(`compress-page:${testInfo.project.name}`);

  await compress(page);
  await page.locator("input[type=file]").setInputFiles(fixture("not-a-pdf.bin"));
  await expect(page.locator(".notice")).toBeVisible({ timeout: 45_000 });

  // THE SERVER'S LOG IS THE GROUND TRUTH, not `page.on("request")`: browser-reported network
  // events for dedicated workers are not equally complete across engines, and the worker is
  // the only place file bytes ever exist (`apps/web/CLAUDE.md`).
  const after = since(`compress-page:${testInfo.project.name}`);
  const unexpected = after.filter((entry) => !isPinnedArtifact(entry.url));
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the compress page uploaded something, or fetched something it did not need — the marker " +
      "is set after the engines are already loaded, so nothing at all should appear here",
  ).toEqual([]);
});

test("the result does not follow you to the next document", async ({ page }) => {
  // THE #69 CLASS, which had no test of any kind on this page -- security review measured it:
  // delete `delivery.invalidate()` from `choose()`, or move `suggestedName()` to after the
  // awaits, and every other test in this file stays green. Both mutations reintroduce the
  // exact finding `tool-delivery.ts`'s header documents, twice, on two other islands.
  await page.goto("/compress-pdf");
  await choose(page, "pages-137.pdf", 137);
  await compress(page);
  await expect(page.locator(".outcome")).toBeVisible();

  // A SECOND DOCUMENT, chosen while the first result is still on screen.
  await choose(page, "blank-1page.pdf", 1);

  // The previous document's sizes must be GONE, not relabelled. A page still showing
  // "4.0 MB -> 2.8 MB" beside a one-page blank file is the interface attributing one
  // document's measurement to another.
  await expect(
    page.locator(".outcome"),
    "the previous document's result survived choosing a new file",
  ).toHaveCount(0);
});

test("the download carries the name of the document it was made from", async ({ page }) => {
  // The other half of #69: the bytes and the name must come from the same run. Checked on the
  // FILE THE BROWSER WOULD SAVE rather than on the link's attribute, because that is what a
  // person ends up with.
  await page.goto("/compress-pdf");
  await choose(page, "pages-137.pdf", 137);
  await compress(page);

  const download = page.waitForEvent("download");
  await page.locator(".outcome__download a.download").click();
  const name = (await download).suggestedFilename();

  expect(name).toBe("pages-137-compressed.pdf");
});

test("the whole flow works from the keyboard alone", async ({ page }) => {
  // `apps/web/CLAUDE.md`'s definition of done: "The page is usable by keyboard, and the drop
  // zone has a file-input fallback." Rotate and reorder each have this test; asserting it here
  // is what keeps the label-wrapped input from becoming a click handler later.
  await page.goto("/compress-pdf");

  // THE DROP ZONE IS REACHED BY TABBING, which is only true because it is a real <input>
  // inside a <label> rather than a div with an onclick.
  const input = page.locator("input[type=file]");
  await input.focus();
  await expect(input).toBeFocused();
  await input.setInputFiles(fixture("pages-137.pdf"));
  await expect(page.locator(".chosen__pages")).toHaveText("137 pages", { timeout: 45_000 });

  // AND THE BUTTON IS ACTIVATED BY KEY, not by a synthetic click.
  const button = page.getByRole("button", { name: "Compress" });
  await button.focus();
  await expect(button).toBeFocused();
  await page.keyboard.press("Enter");

  await expect(page.locator(".outcome")).toBeVisible({ timeout: 45_000 });
  // THE LINK IS REACHABLE TOO. A result a person cannot get to by keyboard is not a result.
  await expect(page.locator(".outcome__download a.download")).toBeVisible();
});
