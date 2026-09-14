// The reorder tool, driven the way a person drives it, in all three browsers.
//
// Everything below `/reorder-pdf` is covered somewhere cheaper: the permutation and the limit
// ordering by `cargo test`, the typed outcomes by the differential corpus, the order grammar by
// `src/components/reorder-order.test.ts`, and the sentences by
// `src/components/reorder-messages.test.ts`. What only a browser can answer is whether the
// pieces are wired together — whether choosing a file reaches the worker, whether the order a
// person typed is the order the pages come out in, whether the download is a document, and
// whether a refusal is something they can read and act on.
//
// WHAT IS ASSERTED ABOUT THE OUTPUT, and why it is not "the page looked right"
//
// The pages' rotations are read back out of the downloaded bytes **in order**. That is the
// same observable the conformance corpus uses, and for the same reason: a page count cannot
// see a permutation — a reorder that did nothing produces a document with exactly as many
// pages as a correct one — so asserting the download is a PDF with four pages would pass
// against a tool that copied the file unchanged.
//
// `mixed-rotation-4page.pdf` is the fixture because its pages are DISTINGUISHABLE: page 1
// carries `/Rotate 270` and the rest inherit 90. Reversing it must turn [270, 90, 90, 90] into
// [90, 90, 90, 270], which fails for a no-op and for any permutation that does not put page 1
// last. A fixture whose pages are interchangeable cannot fail an order test, which is the
// mistake `add-operation` §2c records twice over.

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

/** Choose one file and wait until it has been counted. */
async function choose(page: Page, name: string): Promise<void> {
  await page.locator("input[type=file]").setInputFiles(fixture(name));
  await expect(page.locator(".chosen__pages")).not.toContainText("counting", {
    timeout: 45_000,
  });
}

/**
 * Reorder, then take the download the way a person does — by activating the link.
 *
 * The page does NOT start a download when the operation finishes, deliberately: a tool that
 * writes to somebody's disk without being asked is doing something they did not request.
 */
async function reorderAndDownload(page: Page) {
  await page.getByRole("button", { name: "Put pages in order" }).click();
  const link = page.locator(".result a.download");
  await expect(link).toBeVisible({ timeout: 45_000 });
  const download = page.waitForEvent("download");
  await link.click();
  return await download;
}

async function bytesOf(download: Awaited<ReturnType<typeof reorderAndDownload>>): Promise<Buffer> {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

/**
 * Every value of `key` in the document, in the order it appears in the file.
 *
 * WHY FILE ORDER IS PAGE ORDER HERE, and why the baseline below is what keeps it honest. A
 * real permutation makes qpdf flatten the page tree and renumber objects as it walks it, so
 * pages are written in page order. That is an assumption about qpdf's writer; the test that
 * reads an IDENTITY-written copy of the fixture asserts the order it produces, so the
 * assumption fails loudly rather than silently.
 *
 * Streams are inflated as well as searched raw, because which of the two applies is qpdf's
 * decision rather than ours — the same reason `rotate-pdf.spec.ts` does both.
 */
function sequenceOf(pdf: Buffer, key: RegExp): number[] {
  const found: { at: number; value: number }[] = [];
  const text = pdf.toString("latin1");

  const collect = (haystack: string, bias: number) => {
    for (const match of haystack.matchAll(key)) {
      found.push({ at: bias + (match.index ?? 0), value: Number(match[1]) });
    }
  };

  collect(text, 0);
  for (const match of text.matchAll(/stream\r?\n/g)) {
    const start = (match.index ?? 0) + match[0].length;
    const end = pdf.indexOf("endstream", start, "latin1");
    if (end < 0) continue;
    try {
      collect(inflateSync(pdf.subarray(start, end)).toString("latin1"), start);
    } catch {
      // Not a deflate stream, or not one of ours. Neither is an error here.
    }
  }

  return found.sort((a, b) => a.at - b.at).map((f) => f.value);
}

/**
 * Each page's `/MediaBox` width, in page order. **The identity of a page.**
 *
 * THIS IS THE OBSERVABLE, and `/Rotate` is not — which code review measured rather than
 * argued. `mixed-rotation-4page.pdf` has `/Rotate 270` on page 1 and 90 inherited by the
 * rest, so a rotation sequence distinguishes ONE page out of four: `4,3,2,1`, `4,2,3,1`,
 * `2,3,4,1` and `3,2,4,1` all produce `[90, 90, 90, 270]`. A test asserting that measures
 * where page 1 ended up and nothing else.
 *
 * The widths are distinct per page — `pdf_with_page_tree` gives page `i` the width `101 + i`
 * for exactly this reason, and the Rust side reads them the same way. `4,2,3,1` gives
 * `[104, 102, 103, 101]`, which no other permutation produces.
 */
function pageWidths(pdf: Buffer): number[] {
  return sequenceOf(pdf, /\/MediaBox\s*\[\s*\d+\s+\d+\s+(\d+)/g);
}

/** Every `/Rotate` value, in file order. A secondary check: see `pageWidths`. */
function rotations(pdf: Buffer): number[] {
  return sequenceOf(pdf, /\/Rotate\s+(\d+)/g);
}

test("the fixture's pages are distinguishable, and the reader can tell them apart", () => {
  // THE BASELINE, AND IT ASSERTS AN ORDER. The first version checked only that the reader
  // found *something* and that two distinct rotations existed — which cannot falsify "file
  // order is page order", the assumption everything below rests on. Worse, on the raw fixture
  // the first `/Rotate` it found belonged to the ROOT `/Pages` node rather than to any page.
  // Code review measured both. Found by code review.
  const before = readFileSync(fixture("mixed-rotation-4page.pdf"));
  const widths = pageWidths(before);

  // Four pages, four distinct widths, in the document's own order. If a future fixture change
  // made the pages interchangeable, every order assertion below would go vacuous and this is
  // the test that says so.
  expect(widths).toEqual([101, 102, 103, 104]);
});

test("the pages come out in the order that was typed", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "mixed-rotation-4page.pdf");

  await page.locator(".choice__pages").fill("4-1");
  const bytes = await bytesOf(await reorderAndDownload(page));

  // REVERSED, asserted on the WIDTHS -- the per-page identity. This fails for a no-op and for
  // every one of the other 23 permutations, which a rotation sequence would not.
  expect(pageWidths(bytes)).toEqual([104, 103, 102, 101]);

  // AND WHAT EACH PAGE DISPLAYS SURVIVED. Page 1 carried `/Rotate 270` and the other three
  // inherited 90 from the root; a real permutation flattens the tree and pushes the inherited
  // value onto every page first (ADR 0021). If that ever stopped happening, reordering would
  // silently unrotate three pages of somebody's scan.
  expect(rotations(bytes)).toEqual([90, 90, 90, 270]);
});

test("moving one page to the front moves only that page", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "mixed-rotation-4page.pdf");

  // The completion rule, end to end: type one page, the rest keep their place.
  await page.locator(".choice__pages").fill("2");
  const bytes = await bytesOf(await reorderAndDownload(page));

  // Page 2 first, then 1, 3, 4 in their existing order. On the widths this is exact; on the
  // rotations `[90, 270, 90, 90]` would also be produced by `3,1,2,4` and by `4,1,3,2`.
  expect(pageWidths(bytes)).toEqual([102, 101, 103, 104]);
});

test("the Reverse button types an order, and the order is what runs", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "mixed-rotation-4page.pdf");

  await page.getByRole("button", { name: "Reverse the order" }).click();
  // TYPED INTO THE BOX, not set as a hidden mode: what the preset did is visible and editable.
  await expect(page.locator(".choice__pages")).toHaveValue("4-1");

  expect(pageWidths(await bytesOf(await reorderAndDownload(page)))).toEqual([104, 103, 102, 101]);
});

test("the preview shows the order before anything is run", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "pages-10.pdf");

  await page.locator(".choice__pages").fill("3");
  // THE COMPLETION RULE, SHOWN. This is the thing that keeps it a rule rather than a guess:
  // a person sees the result before running it.
  await expect(page.locator(".choice__preview")).toContainText("3, 1, 2, 4");
});

test("an order that changes nothing is refused by the page, with the reason", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "pages-10.pdf");

  await page.locator(".choice__pages").fill("1-10");
  await expect(page.locator(".choice__preview")).toContainText("already in");
  // The core would accept this happily — it is a legitimate request and a no-op. Running it
  // would rewrite somebody's file to no effect and hand them a download that differs in every
  // byte while displaying identically.
  await expect(page.getByRole("button", { name: "Put pages in order" })).toBeDisabled();
});

test("a second reorder with a different order replaces the first", async ({ page }) => {
  // THE `resultRequest` MECHANISM'S ONLY CONTROL ON THIS PAGE, and it was missing: the island
  // inherited the mechanism from rotate along with the comment recording the ten e2e failures
  // that found its first implementation, and did not inherit the test. A copy that keeps a
  // safety mechanism and drops the test for it is worse than one that keeps neither. Found by
  // code review.
  await page.goto("/reorder-pdf");
  await choose(page, "mixed-rotation-4page.pdf");

  await page.locator(".choice__pages").fill("4-1");
  expect(pageWidths(await bytesOf(await reorderAndDownload(page)))).toEqual([104, 103, 102, 101]);

  // A different order. The finished result is about a request nobody is making any more, so
  // the link must go -- its filename is identical either way, which is exactly why it must.
  await page.locator(".choice__pages").fill("2");
  await expect(page.locator(".result a.download")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Put pages in order" })).toBeEnabled();

  // And running it gives the SECOND order, not the first.
  expect(pageWidths(await bytesOf(await reorderAndDownload(page)))).toEqual([102, 101, 103, 104]);
});

test("editing the order while it runs hides the link rather than mislabelling it", async ({
  page,
}) => {
  // THE DEFECT SECURITY REVIEW FOUND, as a test. `resultRequest` is what decides whether the
  // download link is stale, and it used to be read back AFTER the awaits -- from a `$derived`
  // over the live box. So editing the order mid-run recorded what the box said when the reply
  // landed, the link was judged fresh, and it sat under a preview showing an order the bytes
  // were not in. One permutation looks exactly like another from the outside.
  //
  // A bigger document, deliberately: the edit has to land while the operation is still
  // running, and a four-page file finishes too quickly to be sure of that.
  await page.goto("/reorder-pdf");
  await choose(page, "pages-137.pdf");

  // THE SLOW ORDER IS THE ONE POSTED. A full reversal of 137 pages is 137 removals and 137
  // insertions; `5, 3` is two moves and finishes before an edit can land, which is how the
  // first version of this test passed against the defect it was written for.
  await page.locator(".choice__pages").fill("137-1");
  await page.getByRole("button", { name: "Put pages in order" }).click();
  await expect(page.locator(".working")).toBeVisible();

  // Change the request while it is in flight. What arrives is for the reversal.
  await page.locator(".choice__pages").fill("5, 3");
  await expect(page.locator(".choice__preview")).toContainText("5, 3, 1, 2");

  // The preview now describes a DIFFERENT order from the one that was posted, so there must be
  // no download offered for it. Before the fix the link appeared here, labelled as though it
  // were the order on screen.
  await expect(page.locator(".working")).toHaveCount(0, { timeout: 45_000 });
  await expect(page.locator(".result a.download")).toHaveCount(0);

  // And typing the posted order back in brings it back -- the guard is a comparison, not a
  // one-way latch, so this is what tells the fix from simply never showing the link.
  await page.locator(".choice__pages").fill("137-1");
  await expect(page.locator(".result a.download")).toBeVisible({ timeout: 45_000 });
});

test("the order does not follow you to the next document", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "pages-137.pdf");
  await page.locator(".choice__pages").fill("9-5");
  await expect(page.locator(".choice__preview")).toContainText("9, 8, 7, 6, 5");

  // A different document. Carrying the order over would leave a valid-looking preview for a
  // file the person has not looked at.
  await choose(page, "mixed-rotation-4page.pdf");
  await expect(page.locator(".choice__pages")).toHaveValue("");
});

test("a page listed twice is refused before anything is sent", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "pages-10.pdf");

  await page.locator(".choice__pages").fill("3, 3");
  await expect(page.locator(".choice__problem")).toContainText("listed twice");
  await expect(page.getByRole("button", { name: "Put pages in order" })).toBeDisabled();
});

test("a page the document does not have is refused with the count", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await choose(page, "mixed-rotation-4page.pdf");

  await page.locator(".choice__pages").fill("9");
  await expect(page.locator(".choice__problem")).toContainText("4 pages");
  await expect(page.getByRole("button", { name: "Put pages in order" })).toBeDisabled();
});

test("a file burrow cannot read is refused, and nothing from it reaches the page", async ({
  page,
}) => {
  await page.goto("/reorder-pdf");
  await choose(page, "not-a-pdf.bin");

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible();
  await expect(notice).toContainText("could not read");
  // NO ENGINE PROSE, and no bytes from the file. The page says what the core's typed error
  // means; it never echoes what the engine said about the document.
  await expect(notice).not.toContainText("qpdf");
  await expect(notice).not.toContainText("offset");
});

test("the whole flow works from the keyboard alone", async ({ page }) => {
  await page.goto("/reorder-pdf");
  await page.locator("input[type=file]").setInputFiles(fixture("mixed-rotation-4page.pdf"));
  await expect(page.locator(".chosen__pages")).not.toContainText("counting", { timeout: 45_000 });

  await page.locator(".choice__pages").focus();
  await page.keyboard.type("4-1");
  await expect(page.locator(".choice__preview")).toContainText("4, 3, 2, 1");

  // Tab to the button rather than clicking it.
  const button = page.getByRole("button", { name: "Put pages in order" });
  await button.focus();
  await expect(button).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator(".result a.download")).toBeVisible({ timeout: 45_000 });
});

test("the tool page's console stays empty through a reorder and through a refusal", async ({
  page,
}) => {
  const noise: string[] = [];
  page.on("console", (m) => {
    if (!isBrowserPolicyReport(m.text())) noise.push(`${m.type()}: ${m.text()}`);
  });
  page.on("pageerror", (e) => noise.push(`pageerror: ${e.message}`));

  await page.goto("/reorder-pdf");
  await choose(page, "mixed-rotation-4page.pdf");
  await page.locator(".choice__pages").fill("4-1");
  await reorderAndDownload(page);

  await choose(page, "not-a-pdf.bin");
  await expect(page.locator(".notice")).toBeVisible();

  expect(noise, `the page wrote to the console: ${noise.join(", ")}`).toEqual([]);
});

test("reordering on the tool page makes no request beyond the pinned engine artifacts", async ({
  page,
}, testInfo) => {
  await page.goto("/reorder-pdf");
  // The engine fetches happen on the FIRST file and are legitimate. The marker goes after
  // them, so everything counted below happens on a page whose engines are already loaded --
  // the same discipline `zero-requests.spec.ts` and the rotate page use.
  await choose(page, "mixed-rotation-4page.pdf");
  await mark(`reorder-page:${testInfo.project.name}`);

  await page.locator(".choice__pages").fill("4-1");
  await reorderAndDownload(page);
  await choose(page, "not-a-pdf.bin");

  // THE SERVER'S LOG IS THE GROUND TRUTH, not `page.on("request")`: browser-reported network
  // events for dedicated workers are not equally complete across engines, and the worker is
  // the only place file bytes ever exist (`apps/web/CLAUDE.md`).
  const after = since(`reorder-page:${testInfo.project.name}`);
  const unexpected = after.filter((entry) => !isPinnedArtifact(entry.url));
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the reorder page uploaded something, or fetched something it did not need — the marker " +
      "is set after the engines are already loaded, so nothing at all should appear here",
  ).toEqual([]);
});
