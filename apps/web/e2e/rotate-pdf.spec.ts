// The rotate tool, driven the way a person drives it, in all three browsers.
//
// Everything below `/rotate-pdf` is covered somewhere cheaper: the inheritance walk and the
// limit ordering by `cargo test`, the typed outcomes by the differential corpus, the selection
// grammar by `src/components/page-selection.test.ts`, and the sentences by
// `src/components/rotate-messages.test.ts`. What only a browser can answer is whether the
// pieces are wired together — whether choosing a file reaches the worker, whether the pages a
// person typed are the pages that turn, whether the download is a document, and whether a
// refusal is something they can read and act on.
//
// WHAT IS ASSERTED ABOUT THE OUTPUT, and why it is not "the page looked right"
//
// The downloaded bytes are inflated and `/Rotate` is counted in them. A weaker version was
// available and rejected: asserting that the download exists and is a PDF says nothing about
// whether anything turned, and would pass against a tool that copied the file unchanged.
// Counting the rotations is what tells a rotation from a copy, and counting them per selection
// is what tells "the pages you asked for" from "all of them".
//
// WHAT IS NOT HERE: the page ceiling. A first draft had a test called "a document over the
// page ceiling is refused with both numbers" that chose a 137-page fixture and asserted it
// counted 137 pages -- nothing to do with a ceiling of 10,000, which no fixture in this corpus
// can reach. It was a test whose name promised a refusal and whose body measured a success.
// The ceiling is covered where it can actually be reached: `core/burrow-ops/tests/rotate.rs`
// drives it with a constructed `Limits`, and the conformance corpus compares the refusal
// across both implementations.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";

import { expect, test, type Page } from "@playwright/test";

import { isBrowserPolicyReport } from "./console-noise";
import { isPinnedArtifact, mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "../../../tests/conformance/fixtures");

function fixture(name: string): string {
  return join(fixtures, name);
}

/**
 * Choose one file and wait until it has been counted.
 *
 * "The count settled" is the honest wait rather than a timeout: the page counts through the
 * worker as soon as a file is chosen, and a file that cannot be read settles too, as a refusal.
 * 45s, not 60 — the per-test timeout is 60s, so an inner bound leaves room for a readable
 * failure rather than a bare "test timed out".
 */
async function choose(page: Page, name: string): Promise<void> {
  await page.locator("input[type=file]").setInputFiles(fixture(name));
  await expect(page.locator(".chosen__pages")).not.toContainText("counting", {
    timeout: 45_000,
  });
}

/**
 * Turn, then take the download the way a person does — by activating the link.
 *
 * The page does NOT start a download when the rotation finishes, deliberately: a tool that
 * writes to somebody's disk without being asked is doing something they did not request. So
 * the browser's own download event fires only on activation.
 */
async function turnAndDownload(page: Page) {
  await page.getByRole("button", { name: "Turn pages" }).click();
  const link = page.locator(".result a.download");
  await expect(link).toBeVisible({ timeout: 45_000 });
  const download = page.waitForEvent("download");
  await link.click();
  return await download;
}

/** The bytes behind a download, as a Buffer. */
async function bytesOf(download: Awaited<ReturnType<typeof turnAndDownload>>): Promise<Buffer> {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

/**
 * How many `/Rotate <degrees>` the document carries.
 *
 * qpdf writes page dictionaries into compressed object streams, so a plain text search over
 * the file finds nothing. Every stream is inflated and searched, and the raw text is searched
 * too for the uncompressed case — neither alone is enough, and which one applies is qpdf's
 * decision rather than ours.
 */
function rotationsIn(pdf: Buffer, degrees: number): number {
  const needle = new RegExp(`/Rotate\\s+${degrees}(?![0-9])`, "g");
  let found = [...pdf.toString("latin1").matchAll(needle)].length;

  const text = pdf.toString("latin1");
  for (const match of text.matchAll(/stream\r?\n/g)) {
    const start = match.index + match[0].length;
    const end = pdf.indexOf("endstream", start, "latin1");
    if (end < 0) continue;
    try {
      const inflated = inflateSync(pdf.subarray(start, end)).toString("latin1");
      found += [...inflated.matchAll(needle)].length;
    } catch {
      // Not a deflate stream, or not one of ours. Neither is an error here.
    }
  }
  return found;
}

test("the fixture carries no rotation of its own, so a count of them means something", async () => {
  // THE BASELINE, MEASURED RATHER THAN ASSUMED. `rotationsIn` discriminates a rotation from
  // a copy only because `pages-10.pdf` has no `/Rotate` in it. If a future fixture change
  // gave it one, every count below would be off by ten and the suite would report a
  // rotation that did not happen. Found by code review.
  const before = readFileSync(fixture("pages-10.pdf"));
  for (const degrees of [90, 180, 270]) {
    expect(rotationsIn(before, degrees), `the input already carries /Rotate ${degrees}`).toBe(0);
  }
});

test("choose a PDF, turn every page, and download a document that says so", async ({ page }) => {
  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");

  // The count is the one thing that was working end to end before this page existed, so it is
  // asserted exactly rather than as "a number appeared".
  await expect(page.locator(".chosen__pages")).toContainText("10 pages");

  const download = await turnAndDownload(page);
  expect(download.suggestedFilename()).toBe("pages-10-rotated.pdf");

  const bytes = await bytesOf(download);
  expect(bytes.subarray(0, 5).toString("latin1"), "not a PDF").toBe("%PDF-");
  // TEN PAGES TURNED, not "a download happened". A tool that copied the file unchanged would
  // satisfy every assertion above this line.
  expect(rotationsIn(bytes, 90), "every page should carry /Rotate 90").toBe(10);
});

test("only the pages a person typed are turned", async ({ page }) => {
  // THE ASSERTION THE WHOLE SELECTION BOX EXISTS FOR. "It produced a document" passes for a
  // tool that turns everything; the count is what separates the two.
  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");

  await page.getByLabel("Page numbers to turn", { exact: false }).fill("2-4, 9");
  const download = await turnAndDownload(page);
  const bytes = await bytesOf(download);

  expect(rotationsIn(bytes, 90), "four pages were named, so four should be turned").toBe(4);
});

// EVERY DIRECTION, not just one. A first version checked 180 only, so the 270 option could
// have been mislabelled or wired to the wrong value and nothing would have noticed.
for (const { label, degrees } of [
  { label: "Right, a quarter turn", degrees: 90 },
  { label: "Upside down", degrees: 180 },
  { label: "Left, a quarter turn", degrees: 270 },
]) {
  test(`the direction is the one that was chosen: ${label}`, async ({ page }) => {
    await page.goto("/rotate-pdf");
    await choose(page, "pages-10.pdf");

    await page.getByLabel(label).check();
    const bytes = await bytesOf(await turnAndDownload(page));

    expect(rotationsIn(bytes, degrees), `every page should be ${degrees}`).toBe(10);
    for (const other of [90, 180, 270].filter((d) => d !== degrees)) {
      expect(rotationsIn(bytes, other), `nothing should be ${other}`).toBe(0);
    }
  });
}

test("the Every page control returns to every page after a selection", async ({ page }) => {
  // NOTHING CLICKED IT. Every test that needs "some" reaches it through the box's `onfocus`,
  // and the tests that need "all" rely on the default -- so a `scope` binding that could
  // never return to `"all"` was invisible. Found by code review.
  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");

  await page.getByLabel("Page numbers to turn", { exact: false }).fill("2");
  await page.getByLabel("Every page").check();
  const bytes = await bytesOf(await turnAndDownload(page));

  expect(rotationsIn(bytes, 90), "Every page was chosen after a selection").toBe(10);
});

test("a second rotation with a different selection replaces the first", async ({ page }) => {
  // A FINISHED ROTATION IS OVER WHEN THE REQUEST CHANGES. Without that, "Turn pages" stayed
  // disabled after the first run and the old download link stayed on screen with a filename
  // that still looked right. Found by code review; this is the measurement.
  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");

  await page.getByLabel("Page numbers to turn", { exact: false }).fill("1-3");
  const first = await bytesOf(await turnAndDownload(page));
  expect(rotationsIn(first, 90)).toBe(3);

  await page.getByLabel("Page numbers to turn", { exact: false }).fill("1-7");
  // The stale result is gone the moment the request changes, and the control is live again.
  await expect(page.locator(".result a.download")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Turn pages" })).toBeEnabled();

  const second = await bytesOf(await turnAndDownload(page));
  expect(rotationsIn(second, 90), "the second request is the one that ran").toBe(7);
});

test("a selection that cannot be used is explained beside the box, and blocks the turn", async ({
  page,
}) => {
  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");

  await page.getByLabel("Page numbers to turn", { exact: false }).fill("11");
  await expect(page.locator("#selection-problem")).toContainText("10 pages");
  await expect(page.getByRole("button", { name: "Turn pages" })).toBeDisabled();

  // And it recovers: the problem is about what is typed, not a state the page gets stuck in.
  await page.getByLabel("Page numbers to turn", { exact: false }).fill("1-3");
  await expect(page.locator("#selection-problem")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Turn pages" })).toBeEnabled();
});

test("a file burrow cannot read is refused, and nothing from it reaches the page", async ({
  page,
}) => {
  await page.goto("/rotate-pdf");
  await choose(page, "not-a-pdf.bin");

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible();
  await expect(notice).toContainText("could not read");

  // NOTHING FROM THE FILE, AND NOTHING FROM THE ENGINE. An engine message carrying a byte
  // offset or an object number is file content in every way that matters
  // (`apps/web/CLAUDE.md`).
  //
  // THE FIRST VERSION OF THIS ASSERTION COULD NOT FAIL. It checked that the page did not
  // contain the string "not-a-pdf-content" -- which is not in the fixture, not in any
  // engine's output, and not anywhere else. qpdf could have printed its own name, an offset
  // and an object number into the notice and it would still have passed, under a comment
  // saying it checked exactly that. Found by security review. It now uses the same token
  // list `merge-pdf.spec.ts` uses, plus the fixture's real first bytes, so it has a positive
  // relationship to the file on disk.
  //
  // `section.tool` rather than `body`: the page's own prose legitimately says "PDF" and
  // "burrow", and scanning the whole document would mean choosing tokens to avoid the page
  // rather than tokens that would catch a leak.
  const shown = (await page.locator("section.tool").textContent()) ?? "";
  for (const word of ["offset", "xref", "qpdf", "pdfium", "Malformed", "Error", "GIF89a"]) {
    expect(
      shown,
      `the tool showed "${word}", which comes from the engine or the file`,
    ).not.toContain(word);
  }
  await expect(page.getByRole("button", { name: "Turn pages" })).toBeDisabled();
});

test("the whole flow works from the keyboard alone", async ({ page }) => {
  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");

  // Tab to the selection box and type, then activate the button by keyboard. The drop zone's
  // file input is a real input inside a label, so it is reachable the same way -- setting
  // files is the one thing Playwright cannot do through the keyboard.
  const box = page.getByLabel("Page numbers to turn", { exact: false });
  await box.focus();
  await page.keyboard.type("1-2");
  await expect(page.getByRole("button", { name: "Turn pages" })).toBeEnabled();

  await page.getByRole("button", { name: "Turn pages" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.locator(".result a.download")).toBeVisible({ timeout: 45_000 });
});

test("the tool page's console stays empty through a rotation and through a refusal", async ({
  page,
}) => {
  // THE PAGE, NOT THE HARNESS. `console-silence.spec.ts` drives `/harness`, which is deleted
  // from production builds -- so it says nothing about a route a person can visit.
  const noise: string[] = [];
  page.on("console", (message) => {
    if (!isBrowserPolicyReport(message.text())) noise.push(`${message.type()}: ${message.text()}`);
  });
  page.on("pageerror", (error) => noise.push(`pageerror: ${error.message}`));

  await page.goto("/rotate-pdf");
  await choose(page, "pages-10.pdf");
  await turnAndDownload(page);

  await choose(page, "not-a-pdf.bin");
  await expect(page.locator(".notice")).toBeVisible();

  expect(noise, `the page logged: ${noise.join(" | ")}`).toEqual([]);
});

test("rotating on the tool page makes no request beyond the pinned engine artifacts", async ({
  page,
}, testInfo) => {
  await page.goto("/rotate-pdf");
  // The first file is what STARTS the worker and fetches the engines, and those fetches are
  // legitimate. The marker goes AFTER them, so everything counted below happens on a page
  // whose engines are already loaded -- the same discipline `zero-requests.spec.ts` uses.
  await choose(page, "pages-10.pdf");
  await mark(`rotate-page:${testInfo.project.name}`);

  await turnAndDownload(page);
  await choose(page, "not-a-pdf.bin");

  // THE SERVER'S LOG IS THE GROUND TRUTH, not `page.on("request")`: browser-reported network
  // events for dedicated workers are not equally complete across engines, and the worker is
  // the only place file bytes ever exist (`apps/web/CLAUDE.md`).
  const after = since(`rotate-page:${testInfo.project.name}`);
  const unexpected = after.filter((entry) => !isPinnedArtifact(entry.url));
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the rotate page uploaded something, or fetched something it did not need — the marker is " +
      "set after the engines are already loaded, so nothing at all should appear here",
  ).toEqual([]);
});
