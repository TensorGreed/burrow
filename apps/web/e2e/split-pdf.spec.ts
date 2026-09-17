// The split tool, driven the way a person drives it, in all three browsers.
//
// Everything below `/split-pdf` is covered somewhere cheaper: the pruning rule and the limit
// ordering by `cargo test`, the typed outcomes by the differential corpus, the cut grammar by
// `src/components/split-cuts.test.ts`, the sentences by `src/components/split-messages.test.ts`,
// and the all-or-nothing delivery by `src/host/worker-host.test.ts`. What only a browser can
// answer is whether the pieces are wired together — whether choosing a file reaches the worker,
// whether SEVERAL documents come back, whether each one holds the pages its own name claims,
// and whether a refusal is something a person can read and act on.
//
// WHAT IS ASSERTED ABOUT THE OUTPUT, and why it is not "three downloads happened"
//
// Two things, because either alone is satisfiable by something wrong:
//
//   * EACH PART HOLDS THE NUMBER OF PAGES ITS OWN FILENAME CLAIMS. The name is derived from
//     the cut list the page holds; the bytes are produced from the cut list the core
//     validated. Reading the span back out of the name and counting pages in the bytes is the
//     one assertion that fails if those ever disagree — which is the "bytes under the wrong
//     name" class (#69) in the one operation that produces more than one document. A test that
//     asserted "three files, 3 + 4 + 3 pages" from a hardcoded list would pass while every
//     file was named for somebody else's pages.
//   * THE RIGHT PAGES ARE IN THE RIGHT PART, which page counts cannot see. `pages-10.pdf`'s
//     pages are interchangeable, so a split that returned the correct COUNTS in the wrong
//     ORDER would pass everything above. `mixed-rotation-4page.pdf` carries an inherited
//     `/Rotate 90` on its page-tree root with page 1 overriding it to 270, so the rotation is
//     an observable that says WHICH page a part holds — and the inherited half is the case a
//     flattening would lose.
//
// WHAT IS NOT HERE: the page and size ceilings. No fixture in this corpus reaches 10,000 pages
// or 512 MB, so a test named for a ceiling here would measure a success. Those live in
// `core/burrow-ops/tests/split.rs`, driven with a constructed `Limits`.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";

import { expect, test, type Download, type Page } from "@playwright/test";

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
 */
async function choose(page: Page, name: string): Promise<void> {
  await page.locator("input[type=file]").setInputFiles(fixture(name));
  await expect(page.locator(".chosen__pages")).not.toContainText("counting", {
    timeout: 45_000,
  });
}

/** Type where to cut. */
async function cutAfter(page: Page, cuts: string): Promise<void> {
  await page.getByLabel("The pages to cut after", { exact: false }).fill(cuts);
}

/**
 * Split, then take every download the way a person does — by activating each link.
 *
 * The page does NOT start downloads when the split finishes, deliberately: a tool that writes
 * several files to somebody's disk without being asked is doing that several times over. So
 * the browser's own download events fire only on activation.
 */
async function splitAndDownloadAll(page: Page): Promise<Download[]> {
  await page.getByRole("button", { name: "Split", exact: true }).click();
  const links = page.locator(".result a.download");
  await expect(links.first()).toBeVisible({ timeout: 45_000 });

  const downloads: Download[] = [];
  for (let index = 0; index < (await links.count()); index += 1) {
    const waiting = page.waitForEvent("download");
    await links.nth(index).click();
    downloads.push(await waiting);
  }
  return downloads;
}

/** The bytes behind a download, as a Buffer. */
async function bytesOf(download: Download): Promise<Buffer> {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

/** Every `latin1` view of the file: the raw bytes, plus each stream inflated. */
function searchable(pdf: Buffer): string[] {
  const text = pdf.toString("latin1");
  const views = [text];
  for (const match of text.matchAll(/stream\r?\n/g)) {
    const start = (match.index ?? 0) + match[0].length;
    const end = pdf.indexOf("endstream", start, "latin1");
    if (end < 0) continue;
    try {
      views.push(inflateSync(pdf.subarray(start, end)).toString("latin1"));
    } catch {
      // Not a deflate stream, or not one of ours. Neither is an error here.
    }
  }
  return views;
}

/**
 * How many pages the document has.
 *
 * qpdf writes page dictionaries into compressed object streams, so a plain text search finds
 * nothing; every stream is inflated and searched too. The lookahead is what keeps `/Type
 * /Pages` — the page TREE node, of which there is one — from being counted as a page.
 */
function pageCountIn(pdf: Buffer): number {
  const needle = /\/Type\s*\/Page(?![a-zA-Z])/g;
  return searchable(pdf).reduce((total, view) => total + [...view.matchAll(needle)].length, 0);
}

/** Whether the document carries `/Rotate <degrees>` anywhere, inflated streams included. */
function hasRotation(pdf: Buffer, degrees: number): boolean {
  const needle = new RegExp(`/Rotate\\s+${degrees}(?![0-9])`);
  return searchable(pdf).some((view) => needle.test(view));
}

/** The `first-last` page span a part's own filename claims, as numbers. */
function spanFromName(filename: string): { first: number; last: number } {
  const match = /-pages-(\d+)-(\d+)\.pdf$/.exec(filename);
  if (!match) throw new Error(`filename does not state a page span: ${filename}`);
  return { first: Number(match[1]), last: Number(match[2]) };
}

/**
 * Click Split and then Stop, from INSIDE the page.
 *
 * MEASURED, AND THE REASON THIS IS NOT TWO `locator.click()` CALLS: the largest split this
 * corpus can ask for — `pages-137.pdf` cut at every page, 137 parts — finishes in **130 ms** on
 * Firefox. `merge`'s cancel test gets a window of about a second by taking 72 files; split takes
 * one, so its window is roughly eight times smaller and a driver round trip does not reliably
 * win. It did not: this test passed alone and failed inside a full run, first with "element is
 * not stable" and then with "element is not visible" — two spellings of "it finished first".
 *
 * So the wait happens in the page's own event loop, a frame at a time, with no round trip
 * between seeing Stop and pressing it. If Stop never appears the split finished before anything
 * could be pressed, and that is reported as measuring nothing rather than as a cancel.
 */
async function splitThenStop(page: Page): Promise<void> {
  const caught = await page.evaluate(async () => {
    const button = (label: string) =>
      [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === label);
    button("Split")?.click();
    const deadline = performance.now() + 10_000;
    return await new Promise<boolean>((resolve) => {
      const tick = () => {
        const stop = button("Stop");
        if (stop) {
          stop.click();
          resolve(true);
          return;
        }
        if (performance.now() > deadline) {
          resolve(false);
          return;
        }
        requestAnimationFrame(tick);
      };
      tick();
    });
  });
  expect(
    caught,
    "Stop never appeared, so the split finished before it could be pressed and this test " +
      "measured nothing about cancelling — it needs a slower split, not a longer timeout",
  ).toBe(true);
}

test("the fixtures make the assertions below mean something", async () => {
  // THE BASELINE, MEASURED RATHER THAN ASSUMED, and it is two separate claims.
  //
  // `pageCountIn` discriminates a correct partition from a wrong one only if it counts pages
  // rather than page-tree nodes. And `hasRotation` tells parts apart only because page 1 of
  // the mixed fixture carries a rotation the other three do not.
  expect(pageCountIn(readFileSync(fixture("pages-10.pdf"))), "the counter is wrong").toBe(10);

  const mixed = readFileSync(fixture("mixed-rotation-4page.pdf"));
  expect(pageCountIn(mixed)).toBe(4);
  expect(hasRotation(mixed, 270), "page 1's override is what tells the parts apart").toBe(true);
});

test("choose a PDF, cut it, and download parts that hold the pages their names claim", async ({
  page,
}) => {
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");
  await expect(page.locator(".chosen__pages")).toContainText("10 pages");

  await cutAfter(page, "3, 7");
  // THE PREVIEW IS PART OF THE CLAIM. A cut is a gap and not a page, so the page shows the
  // files it will produce; if that ever disagreed with what comes out, the names below would
  // be describing something else.
  await expect(page.locator(".choice__preview")).toContainText("3 files");
  await expect(page.locator(".choice__parts")).toContainText("1-3, 4-7, 8-10");

  const downloads = await splitAndDownloadAll(page);
  expect(downloads, "three parts were asked for").toHaveLength(3);

  const names = downloads.map((d) => d.suggestedFilename());
  // ZERO-PADDED TO THE SOURCE'S WIDTH, so a file manager sorts ten parts in order.
  expect(names).toEqual([
    "pages-10-pages-01-03.pdf",
    "pages-10-pages-04-07.pdf",
    "pages-10-pages-08-10.pdf",
  ]);

  let pagesSeen = 0;
  for (const download of downloads) {
    const name = download.suggestedFilename();
    const bytes = await bytesOf(download);
    expect(bytes.subarray(0, 5).toString("latin1"), `${name} is not a PDF`).toBe("%PDF-");

    // THE NAME IS CHECKED AGAINST THE BYTES, by reading the span out of the name rather than
    // from a list written here. A hardcoded expectation would pass while every file was named
    // for somebody else's pages.
    const { first, last } = spanFromName(name);
    expect(pageCountIn(bytes), `${name} does not hold the pages it claims`).toBe(last - first + 1);
    pagesSeen += last - first + 1;
  }

  // AND THE PARTS ADD UP. `split` is defined as a partition, and three files of the right
  // sizes that between them lost a page would satisfy every assertion above.
  expect(pagesSeen, "the parts do not add up to the document").toBe(10);
});

test("the right pages are in the right part, which page counts cannot see", async ({ page }) => {
  // `pages-10.pdf`'s pages are interchangeable, so a split that returned the correct counts in
  // the wrong order passes the test above. This fixture's page 1 carries `/Rotate 270` over an
  // inherited `/Rotate 90`, so the rotation says WHICH page a part holds.
  //
  // CUT AFTER 1 AND 2, NOT JUST 1, and that is the difference between this test measuring what
  // it claims and measuring nothing extra. Cutting after page 1 alone gives parts of 1 and 3
  // pages, so a swap is already caught by the counts and the rotation assertions add nothing --
  // the header above would be describing a case this test did not rule out. Cutting after 1 and
  // 2 gives 1, 1, 2: the first two parts are indistinguishable by count, and the rotation is the
  // only thing that can tell them apart. Code review found the gap.
  await page.goto("/split-pdf");
  await choose(page, "mixed-rotation-4page.pdf");
  await expect(page.locator(".chosen__pages")).toContainText("4 pages");

  await cutAfter(page, "1, 2");
  const downloads = await splitAndDownloadAll(page);
  expect(downloads).toHaveLength(3);

  const [one, two, rest] = await Promise.all(downloads.map(bytesOf));

  // The first two parts are one page each, so counting cannot separate them.
  expect(pageCountIn(one)).toBe(1);
  expect(pageCountIn(two)).toBe(1);
  expect(pageCountIn(rest)).toBe(2);

  // THE ONLY THING THAT CAN. Page 1's own rotation must be in the part holding page 1, and in
  // neither of the others -- page 2 inherits `/Rotate 90`, so the 270 needle stays clean.
  expect(hasRotation(one, 270), "page 1's rotation is not in the part that holds it").toBe(true);
  expect(hasRotation(two, 270), "page 1's rotation reached the part holding page 2").toBe(false);
  expect(hasRotation(rest, 270), "page 1's rotation reached a part that does not hold it").toBe(
    false,
  );
});

test("a layered document is refused, and the page says why rather than shrugging", async ({
  page,
}) => {
  // THE REFUSAL THIS PAGE IS ABOUT. ADR 0019 §4 is explicit that the reason is the
  // load-bearing half: "a page that said only 'documents with layers cannot be split' would
  // read as a bug". A generic "burrow does not handle that" would pass a weaker assertion.
  //
  // THE DOCUMENT OPENS AND THEN THE SPLIT REFUSES, and that order is asserted rather than
  // accommodated. `tests/conformance/expectations.json`'s `split-refuses-a-layered-document`
  // records `split` → `Unsupported` on this fixture, so the count must succeed first. An
  // earlier version of this test branched on whether the count had failed, which would have
  // gone on passing if the refusal ever moved to the open path -- a test that adapts to
  // behaviour cannot report a change in it.
  await page.goto("/split-pdf");
  await choose(page, "layered.pdf");
  await expect(
    page.locator(".chosen__pages"),
    "the fixture stopped opening, so this no longer tests the SPLIT refusing",
  ).not.toContainText("could not be read");
  await expect(page.locator(".notice")).toHaveCount(0);

  await cutAfter(page, "1");
  await page.getByRole("button", { name: "Split", exact: true }).click();

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible({ timeout: 45_000 });
  await expect(notice).toContainText("layers");
  await expect(notice).toContainText("recorded for the document as a whole");

  // AND NOTHING WAS HANDED OVER. A refusal that still produced links would be the page
  // contradicting itself.
  await expect(page.locator(".result a.download")).toHaveCount(0);
});

test("a file burrow cannot read is refused, and nothing from it reaches the page", async ({
  page,
}) => {
  await page.goto("/split-pdf");
  await choose(page, "truncated.pdf");

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible({ timeout: 45_000 });

  // NOTHING FROM THE FILE, and nothing from the engine either. The engines log object numbers
  // and byte offsets on files that parse successfully, let alone damaged ones.
  const text = (await page.locator("section.tool").textContent()) ?? "";
  for (const leak of ["offset", "xref", "qpdf", "pdfium", "Malformed", "Error", "obj"]) {
    expect(text, `the page echoed "${leak}"`).not.toContain(leak);
  }
  await expect(page.locator(".result a.download")).toHaveCount(0);
});

test("the preset cuts a document into single pages, in one gesture", async ({ page }) => {
  // THE GAP THIS CLOSES, driven the way a person would. Without it, splitting a ten-page
  // document into single pages means typing nine cut points; a forty-page one, thirty-nine.
  // `/reorder-pdf` reverses a 500-page document in five characters, and this page had no
  // equivalent until the range and this button.
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");

  await page.getByRole("button", { name: "Cut after every page" }).click();

  // IT TYPED INTO THE BOX rather than switching to a mode, which is what lets somebody see
  // what was asked for and change one cut afterwards.
  await expect(page.getByLabel("The pages to cut after", { exact: false })).toHaveValue("1-9");
  await expect(page.locator(".choice__preview")).toContainText("10 files");

  const downloads = await splitAndDownloadAll(page);
  expect(downloads).toHaveLength(10);
  expect(downloads.map((d) => d.suggestedFilename())).toEqual(
    Array.from({ length: 10 }, (_, i) => {
      const n = String(i + 1).padStart(2, "0");
      return `pages-10-pages-${n}-${n}.pdf`;
    }),
  );
  for (const download of downloads) {
    expect(pageCountIn(await bytesOf(download)), `${download.suggestedFilename()}`).toBe(1);
  }
});

test("a range in the box is read as a cut after each page in it", async ({ page }) => {
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");

  await cutAfter(page, "1-2, 7");
  await expect(page.locator(".choice__parts")).toContainText("page 1, page 2, 3-7, 8-10");

  const downloads = await splitAndDownloadAll(page);
  expect(downloads.map((d) => d.suggestedFilename())).toEqual([
    "pages-10-pages-01-01.pdf",
    "pages-10-pages-02-02.pdf",
    "pages-10-pages-03-07.pdf",
    "pages-10-pages-08-10.pdf",
  ]);
});

test("a cut past the end is refused beside the box, and the page recovers", async ({ page }) => {
  // REFUSED WHERE THE BOX IS, not after a round trip. A cut after the last page asks for a
  // part with no pages in it; the core refuses it, and so does the page, with the last place
  // that would work.
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");

  await cutAfter(page, "10");
  const problem = page.locator(".choice__problem");
  await expect(problem).toBeVisible();
  await expect(problem).toContainText("after page 9");
  await expect(page.getByRole("button", { name: "Split", exact: true })).toBeDisabled();

  // AND IT RECOVERS. A page that refuses and then stays refused is a dead end.
  await cutAfter(page, "9");
  await expect(problem).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Split", exact: true })).toBeEnabled();
});

test("not cutting at all is refused, because it would hand back the same document", async ({
  page,
}) => {
  // The core accepts an empty cut list -- it is a legitimate one-way split and the identity
  // case its property tests use. The PAGE refuses it, the way /reorder-pdf refuses an
  // identity: running it rewrites somebody's file into a copy of itself.
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");

  await expect(page.locator(".choice__preview")).toContainText("one piece");
  await expect(page.getByRole("button", { name: "Split", exact: true })).toBeDisabled();
});

test("choosing another document mid-split hands over nothing under the old name", async ({
  page,
}) => {
  // THE DISCLOSURE PATH THIS WHOLE DELIVERY LAYER EXISTS FOR (#69), in its multi-output form.
  // Choose a document, start a split, change your mind, choose another: the first split's
  // parts must not arrive under names derived from the file chosen SINCE.
  //
  // IT IS TESTED IN A BROWSER because that is where it happened. The unit tests drive
  // `createDelivery` in isolation and would pass against an island that called `beginParts`
  // AFTER its awaits, which is exactly the defect found twice -- once on rotate, once on
  // reorder. Security review's point: split is the sharpest case and had no test of it.
  await page.goto("/split-pdf");
  await choose(page, "pages-137.pdf");
  await cutAfter(page, Array.from({ length: 40 }, (_, i) => (i + 1) * 3).join(", "));

  // NO WAIT FOR THE MID-FLIGHT WINDOW, deliberately. A 137-part split finishes in 130 ms
  // (measured; see `splitThenStop`), so a test that required Stop to still be on screen would
  // be reporting on timing. The property under test holds in BOTH orderings — whether the
  // split is still running or has just finished, choosing another document must leave no link
  // named for the first — so both are asserted rather than one raced for.
  await page.getByRole("button", { name: "Split", exact: true }).click();
  await choose(page, "pages-10.pdf");

  // The page is now about the second document, and the first split's parts are gone.
  await expect(page.locator(".chosen__name")).toHaveText("pages-10.pdf");
  await expect(
    page.locator(".result a.download"),
    "a link survived a change of document",
  ).toHaveCount(0);

  // AND NO LINK EVER CARRIED THE OLD DOCUMENT'S NAME. Splitting the second file must produce
  // names derived from it alone.
  await cutAfter(page, "4");
  const downloads = await splitAndDownloadAll(page);
  for (const download of downloads) {
    expect(download.suggestedFilename()).toContain("pages-10-pages-");
    expect(download.suggestedFilename()).not.toContain("pages-137");
  }
});

test("editing the cuts while it runs hides the files rather than mislabelling them", async ({
  page,
}) => {
  // The reorder finding, in its multi-output form: the request signature read back AFTER the
  // awaits produced a link the staleness guard judged fresh, sitting under a preview showing
  // something the bytes were not. Here it would be a SET of names describing cuts that did
  // not run.
  await page.goto("/split-pdf");
  await choose(page, "pages-137.pdf");
  // EVERY PAGE ITS OWN PART: the longest split this corpus can ask for, so the window in which
  // the box can be edited is as wide as it gets. With forty parts Firefox finished before the
  // edit landed and the completion path announced the ordinary "ready to download" — a
  // different sequence from the one this test is named for, reached intermittently.
  await cutAfter(page, Array.from({ length: 136 }, (_, i) => i + 1).join(", "));

  // Again no wait for the window: the property holds whether the edit lands during the split
  // or just after it. What must never happen is a link describing cuts that did not run.
  await page.getByRole("button", { name: "Split", exact: true }).click();
  await cutAfter(page, "10, 20");

  // Nothing is offered, because what was produced is not the answer to what the box says.
  await expect(page.locator(".result a.download")).toHaveCount(0);
  await expect(page.locator(".result")).toHaveCount(0);

  // WHAT IS NOT ASSERTED HERE, and why. The completion path announces a different sentence
  // when the result is finished but stale — "the cuts changed while it ran", instead of
  // telling a screen-reader user that files are ready when none are shown (security review).
  // Which sentence fires depends on whether the edit landed before or after the split
  // finished, and at 130 ms that is not something this suite can decide. Asserting it would
  // be asserting a race. The behaviour is in `SplitTool.svelte`'s completion path; the
  // sentence itself is not covered by a test, which is said here rather than left to look
  // covered by the assertions above.

  // Splitting again produces the cuts that are actually in the box.
  const downloads = await splitAndDownloadAll(page);
  expect(downloads.map((d) => d.suggestedFilename())).toEqual([
    "pages-137-pages-001-010.pdf",
    "pages-137-pages-011-020.pdf",
    "pages-137-pages-021-137.pdf",
  ]);
});

test("a second split replaces the first, rather than adding to it", async ({ page }) => {
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");

  await cutAfter(page, "5");
  await splitAndDownloadAll(page);
  await expect(page.locator(".result a.download")).toHaveCount(2);

  await cutAfter(page, "2, 4, 6");
  const downloads = await splitAndDownloadAll(page);

  // EXACTLY THE SECOND SET. Four links, and none of them named for the first split's cuts --
  // a page that appended would show six, and one that reused the old names would show the
  // wrong spans over the right bytes.
  expect(downloads.map((d) => d.suggestedFilename())).toEqual([
    "pages-10-pages-01-02.pdf",
    "pages-10-pages-03-04.pdf",
    "pages-10-pages-05-06.pdf",
    "pages-10-pages-07-10.pdf",
  ]);
  for (const download of downloads) {
    const { first, last } = spanFromName(download.suggestedFilename());
    expect(pageCountIn(await bytesOf(download))).toBe(last - first + 1);
  }
});

test("Stop ends the split, hands over nothing, and does not cost the page its engine", async ({
  page,
}) => {
  await page.goto("/split-pdf");
  // The largest document the corpus has, cut into EVERY page, because a cancel needs something
  // to interrupt and it has to still be running when the click lands. Forty parts was not
  // enough: it passed alone and failed inside a full run, where the engines are warm and the
  // split finished between the "still running" assertion and the click. A flaky cancel test is
  // worse than none — it reports on timing rather than on cancelling.
  await choose(page, "pages-137.pdf");
  await cutAfter(page, Array.from({ length: 136 }, (_, i) => i + 1).join(", "));

  await splitThenStop(page);

  await expect(page.locator('[role="status"]').last()).toHaveText("Stopped.");

  // A CANCELLED SPLIT IS NOT A FAILURE. The host fails the in-flight request with `Internal`
  // when the worker is discarded, which is correct from its point of view and a lie to a
  // person who pressed Stop.
  await expect(
    page.locator(".notice"),
    "a cancelled split reported a failure — the person stopped it, nothing went wrong",
  ).toHaveCount(0);

  // AND NOTHING WAS HANDED OVER. Parts produced before the cancel are held by the host, never
  // delivered (ADR 0023 §3).
  await expect(page.locator(".result a.download")).toHaveCount(0);

  // AND THE BREAKER DID NOT LATCH. A cancel is a respawn, not a crash; counting respawns took
  // the page offline while every worker was healthy (ADR 0015 §3).
  await cutAfter(page, "5");
  await page.getByRole("button", { name: "Split", exact: true }).click();
  await expect(page.locator(".result a.download").first()).toBeVisible({ timeout: 45_000 });
});

test("the whole flow works from the keyboard alone", async ({ page }) => {
  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");

  // The drop zone's file input is a real input inside a label, so it is reachable the same
  // way — setting files is the one thing Playwright cannot do through the keyboard.
  const box = page.getByLabel("The pages to cut after", { exact: false });
  await box.focus();
  await page.keyboard.type("5");
  await expect(page.getByRole("button", { name: "Split", exact: true })).toBeEnabled();

  await page.getByRole("button", { name: "Split", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(page.locator(".result a.download").first()).toBeVisible({ timeout: 45_000 });
});

test("the tool page's console stays empty through a split and through a refusal", async ({
  page,
}) => {
  // AGAINST THE PAGE THAT SHIPS, not `/harness`, which is deleted from production builds — so
  // an assertion that only ran there would say nothing about a route a person can visit.
  const noise: string[] = [];
  page.on("console", (message) => {
    if (!isBrowserPolicyReport(message.text())) noise.push(`${message.type()}: ${message.text()}`);
  });
  page.on("pageerror", (error) => noise.push(`pageerror: ${error.message}`));

  await page.goto("/split-pdf");
  await choose(page, "pages-10.pdf");
  await cutAfter(page, "4");
  await splitAndDownloadAll(page);

  await choose(page, "truncated.pdf");
  await expect(page.locator(".notice")).toBeVisible({ timeout: 45_000 });

  expect(noise, `the page logged: ${noise.join(" | ")}`).toEqual([]);
});

test("a split makes no request beyond the pinned engine artifacts", async ({ page }, testInfo) => {
  // THE SERVER'S ACCEPT LOG IS GROUND TRUTH, not `page.on("request")`: browser-reported
  // network events for dedicated workers are not equally complete across engines, and the
  // worker is the only place file bytes exist.
  await page.goto("/split-pdf");
  // The first file loads the engines, so the mark goes AFTER it — what is being asserted is
  // that an OPERATION is silent, not that the engines never load.
  await choose(page, "pages-10.pdf");

  await mark(`split-page:${testInfo.project.name}`);

  await cutAfter(page, "2, 6");
  const downloads = await splitAndDownloadAll(page);
  expect(downloads).toHaveLength(3);

  const after = since(`split-page:${testInfo.project.name}`);
  // THE WIDENED SET, DELIBERATELY, AND ONLY ON THE TWO PAGES THAT DRAW PICTURES.
  //
  // `isPinnedArtifact` takes the bundle as an argument precisely so this is a decision rather
  // than a default: widening it everywhere would let `/merge-pdf` download 1.9 MB of PDFium
  // and still pass all five tool-page assertions, which is the defect the per-bundle split was
  // built for. `/merge-pdf`, `/reorder-pdf` and `/compress-pdf` stay on the BASE set, where a
  // render fetch is still a failure -- and `merge-pdf.spec.ts` asserts that over a whole visit.
  //
  // This page renders, so the render bundle is an artifact it is entitled to. Everything else
  // is still refused.
  const unexpected = after.filter((entry) => !isPinnedArtifact(entry.url, "all"));
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the split page uploaded something, or fetched something it did not need — the marker is " +
      "set after the engines are already loaded, so nothing at all should appear here",
  ).toEqual([]);
});
