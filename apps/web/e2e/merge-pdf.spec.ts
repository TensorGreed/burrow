// The merge tool, driven the way a person drives it, in all three browsers.
//
// Everything below `/merge-pdf` is already covered somewhere cheaper: the limit ordering and
// the error mapping by `cargo test`, the typed outcomes by the differential corpus
// (`tests/conformance/expectations.json`), and the sentences by
// `src/components/merge-messages.test.ts`. What only a browser can answer is whether the
// pieces are wired together — whether choosing a file reaches the worker, whether the order
// shown is the order merged, whether the download is a document, and whether a refusal is
// something a person can read and act on.
//
// WHAT THIS FILE DOES NOT ASSERT, AND WHERE IT IS ASSERTED INSTEAD
//
// **WHICH page ended up first.** qpdf writes its pages into compressed object streams, so the
// output's page objects cannot be read back out of the downloaded bytes without a PDF parser —
// which is the thing under test. `core/burrow-ops/tests/merge.rs` asserts that natively, by
// reading `/MediaBox` widths from an uncompressed write.
//
// What IS asserted here is the page order of the downloaded document, read by inflating its
// content streams — see `pageOrder`. Two weaker versions were tried and refuted first: the
// downloaded FILENAME (read from the same array the list renders from, so it says nothing
// about the request), and "the two orders produce different bytes" (a mutation reversing
// `blobs` in the island reverses both merges, so they still differ and the test stayed green).
//
// THE PAGE IS THE SUBJECT, NOT THE HARNESS
//
// `console-silence.spec.ts` and `zero-requests.spec.ts` drive `/harness`, which is a test-only
// route that is deleted from production builds. The last two tests here run the same two
// assertions against the page that actually ships.

import { readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { isBrowserPolicyReport } from "./console-noise";
import { ORDER_FIXTURE_DIR } from "./global-setup.mjs";
import { isPinnedArtifact, mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "../../../tests/conformance/fixtures");

function fixture(name: string): string {
  return join(fixtures, name);
}

/** The page's own ceilings, mirrored from `MergeTool.svelte`, so a test can reason in them. */
const MAX_PAGES = 10_000;

/**
 * Choose files through the real `<input type=file>`, and wait until every one is counted.
 *
 * The page counts each file through the worker as it is added, so "the list settled" is the
 * honest wait — not a timeout. A file that cannot be read settles too, as a refusal.
 */
async function choose(page: Page, names: string[], alreadyListed = 0): Promise<void> {
  await page.locator("input[type=file]").setInputFiles(names.map(fixture));
  await expect(page.locator("li.file")).toHaveCount(alreadyListed + names.length);
  // 45s, not 60: the per-test timeout is 60s (`playwright.config.ts`), so an inner bound at
  // the same number can never fire and its message can never be read. This one can.
  await expect(page.locator(".file__pages", { hasText: "counting" })).toHaveCount(0, {
    timeout: 45_000,
  });
}

/**
 * Merge, then take the download the way a person does — by activating the link.
 *
 * The page does NOT start a download when the merge finishes, and that is deliberate: a tool
 * that writes to someone's disk without being asked is doing something they did not request.
 * The result is a link, so the browser's own download event only fires when it is activated.
 */
async function mergeAndDownload(page: Page) {
  await page.getByRole("button", { name: "Merge", exact: true }).click();
  const link = page.locator(".result a.download");
  await expect(link).toBeVisible({ timeout: 45_000 });
  const download = page.waitForEvent("download");
  await link.click();
  return await download;
}

/** The names in the list, in the order the page will merge them. */
function listedNames(page: Page) {
  return page.locator("li.file .file__name");
}

test("pick files, reorder, merge, and download a document with every page in it", async ({
  page,
}) => {
  await page.goto("/merge-pdf");
  await choose(page, ["pages-10.pdf", "pages-137.pdf"]);

  // The page counts are the one thing that was already working end to end before this page
  // existed, so they are asserted exactly rather than as "a number appeared".
  await expect(page.locator("li.file").nth(0)).toContainText("10");
  await expect(page.locator("li.file").nth(1)).toContainText("137");
  await expect(page.locator(".total")).toContainText("147");

  // REORDER, because merge order is the whole point of the tool.
  await page.getByRole("button", { name: "Move pages-137.pdf up" }).click();
  await expect(listedNames(page)).toHaveText(["pages-137.pdf", "pages-10.pdf"]);

  const file = await mergeAndDownload(page);

  // The suggested name comes from the first entry. That is evidence about the LIST, not about
  // the request — `suggestedName()` reads the same array the list renders from. The test
  // below is the one that constrains what was actually sent.
  expect(file.suggestedFilename()).toBe("pages-137-merged.pdf");

  const path = await file.path();
  expect(path, "the download produced no file on disk").not.toBeNull();
  const bytes = readFileSync(path as string);

  // A document, structurally: the header and the trailer marker qpdf writes.
  expect(bytes.subarray(0, 5).toString("latin1")).toBe("%PDF-");
  expect(bytes.subarray(-1024).toString("latin1")).toContain("%%EOF");

  // AND A DOCUMENT THE ENGINE AGREES IS ONE, with every page in it. Read back through the
  // tool itself: a header check would pass on a truncated write, and the page count is the
  // property that would actually be wrong if a source had been released too early — the
  // silent truncation ADR 0017 §2 refuses.
  await page.reload();
  await page.locator("input[type=file]").setInputFiles({
    name: file.suggestedFilename(),
    mimeType: "application/pdf",
    buffer: bytes,
  });
  await expect(page.locator(".total")).toContainText("147", { timeout: 45_000 });
});

/**
 * The pages of a merged document, in the order they appear in it.
 *
 * `plain2.pdf` and `plain3.pdf` (see `e2e/global-setup.mjs`) give every page a content stream
 * that names itself. qpdf deflates those streams on the way out, so they are inflated here --
 * `node:zlib`, no dependency -- and the markers are read off in file order.
 *
 * THIS IS NOT A PDF PARSER and must not grow into one. It finds `stream`/`endstream` pairs and
 * tries to inflate each; anything that does not inflate, or carries no marker, is skipped. That
 * is sound for exactly this job because the fixtures are ours and the markers are unique. The
 * test below asserts the FULL expected sequence, so a reader that silently found nothing would
 * fail rather than pass.
 */
function pageOrder(pdf: Buffer): string[] {
  const markers: string[] = [];
  const text = pdf.toString("latin1");
  for (const match of text.matchAll(/stream\r?\n/g)) {
    const start = match.index + match[0].length;
    const end = pdf.indexOf("endstream", start, "latin1");
    if (end < 0) continue;
    try {
      const inflated = inflateSync(pdf.subarray(start, end)).toString("latin1");
      for (const found of inflated.matchAll(/page \d+ of plain\d\.pdf/g)) markers.push(found[0]);
    } catch {
      // Not a deflate stream, or not one of ours. Neither is an error here.
    }
  }
  return markers;
}

test("the order in the list is the order in the document", async ({ page }) => {
  // WHY NOT A CONFORMANCE FIXTURE: every file in `tests/conformance/fixtures/` has the same
  // empty page at the same size, so merging any two of them in either order produces
  // byte-identical output -- measured at 1,603 bytes and one digest both ways. A test built on
  // those would have "passed" for a reason that had nothing to do with the page.
  //
  // WHY NOT "the two orders differ": that was the first version, and a mutation refuted it.
  // Reversing `blobs` in the island reverses BOTH merges, so the two outputs still differ and
  // the test stayed green. A difference can only catch an order that never left the list, not
  // one that arrived backwards. The sequence is asserted instead.
  const two = join(ORDER_FIXTURE_DIR, "plain2.pdf");
  const three = join(ORDER_FIXTURE_DIR, "plain3.pdf");

  await page.goto("/merge-pdf");
  await page.locator("input[type=file]").setInputFiles([two, three]);
  await expect(page.locator(".total")).toContainText("5", { timeout: 45_000 });

  const forwards = readFileSync((await (await mergeAndDownload(page)).path()) as string);
  expect(pageOrder(forwards)).toEqual([
    "page 1 of plain2.pdf",
    "page 2 of plain2.pdf",
    "page 1 of plain3.pdf",
    "page 2 of plain3.pdf",
    "page 3 of plain3.pdf",
  ]);

  await page.getByRole("button", { name: "Move plain3.pdf up" }).click();
  await expect(listedNames(page)).toHaveText(["plain3.pdf", "plain2.pdf"]);

  const backwards = readFileSync((await (await mergeAndDownload(page)).path()) as string);
  expect(pageOrder(backwards)).toEqual([
    "page 1 of plain3.pdf",
    "page 2 of plain3.pdf",
    "page 3 of plain3.pdf",
    "page 1 of plain2.pdf",
    "page 2 of plain2.pdf",
  ]);
});

test("a second merge works without emptying the list first", async ({ page }) => {
  // `phase` stayed `"done"` after a successful merge and only `reset()` — reached when the
  // list became EMPTY — put it back. So adding a third file to a finished pair rendered Merge
  // disabled with no explanation, beside a download link for a document that no longer
  // matched the list. Found by both reviews; nothing here covered it.
  await page.goto("/merge-pdf");
  await choose(page, ["pages-10.pdf", "blank-1page.pdf"]);
  await mergeAndDownload(page);

  await choose(page, ["pages-137.pdf"], 2);
  await expect(
    page.locator(".result"),
    "the previous merge's download is still offered for a list that has changed",
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Merge", exact: true })).toBeEnabled();

  const again = await mergeAndDownload(page);
  expect(again.suggestedFilename()).toBe("pages-10-merged.pdf");
  await expect(page.locator(".total")).toContainText("148");
});

test("an encrypted input is refused by name, and blocks the merge", async ({ page }) => {
  await page.goto("/merge-pdf");
  await choose(page, ["pages-10.pdf", "encrypted.pdf"]);

  const bad = page.locator("li.file").nth(1);
  await expect(bad).toHaveClass(/file--bad/);
  await expect(bad).toContainText("password-protected");
  // What to do next, which is the half a message is useless without.
  await expect(bad).toContainText("Remove the password");

  // ALL-OR-NOTHING, visible: the good file is not quietly merged on its own (ADR 0017 §2).
  await expect(page.getByRole("button", { name: "Merge", exact: true })).toBeDisabled();
  await expect(page.locator(".total")).toContainText("until every file can be read");

  // And removing it clears the block, so the refusal is a direction rather than a dead end.
  await page.getByRole("button", { name: "Remove encrypted.pdf" }).click();
  await expect(page.getByRole("button", { name: "Merge", exact: true })).toBeEnabled();
});

test("a corrupt input is refused, and nothing from the file reaches the page", async ({ page }) => {
  await page.goto("/merge-pdf");
  await choose(page, ["not-a-pdf.bin", "pages-10.pdf"]);

  const bad = page.locator("li.file").nth(0);
  await expect(bad).toHaveClass(/file--bad/);
  await expect(bad).toContainText("could not read that file");

  // qpdf's own words for this file carry a byte offset, and a byte offset is file content.
  // Nothing of the sort may appear, and neither may the engine's or the variant's name.
  const shown = (await page.locator("section.tool").textContent()) ?? "";
  for (const word of ["offset", "xref", "qpdf", "pdfium", "Malformed", "Error"]) {
    expect(
      shown,
      `"${word}" is engine prose or a variant name, not a sentence for a person`,
    ).not.toContain(word);
  }
});

test("a merge over the page-count ceiling is refused with both numbers", async ({ page }) => {
  await page.goto("/merge-pdf");

  // 74 x 137 pages = 10,138, over the 10,000 the page states in its own prose. The ceiling is
  // NOT re-implemented here: the page sends the files and reports what the core refuses, so
  // this test is also what would catch the two drifting apart.
  const many = Array.from({ length: 74 }, () => "pages-137.pdf");
  await choose(page, many);
  await expect(page.locator(".total")).toContainText("10138");

  await page.getByRole("button", { name: "Merge", exact: true }).click();

  const notice = page.locator(".notice");
  await expect(notice).toBeVisible({ timeout: 45_000 });
  // BOTH NUMBERS. The wrapper `InputFailed` names which input failed and never what was wrong
  // with it, and the binding read only the outer variant until this page was built — so a
  // ceiling arrived as "that is more than burrow will take on" with the limit, the stage and
  // both numbers discarded. Asserting the numbers is what keeps that from coming back.
  await expect(notice).toContainText(String(MAX_PAGES));
  await expect(notice).toContainText("pages");
  await expect(notice, "the ceiling was quoted vaguely rather than exactly").not.toContainText(
    "more than burrow will take on",
  );
});

test("Stop ends the merge, and does not cost the page its engine", async ({ page }) => {
  await page.goto("/merge-pdf");

  // The largest merge the page will accept, because a cancel needs something to interrupt.
  // 72 x 137 = 9,864 pages, which takes around a second — this is the honest upper bound
  // available, and the assertion below fails loudly rather than quietly if it is not enough.
  await choose(
    page,
    Array.from({ length: 72 }, () => "pages-137.pdf"),
  );

  await page.getByRole("button", { name: "Merge", exact: true }).click();

  const stop = page.getByRole("button", { name: "Stop", exact: true });
  await expect(
    stop,
    "the merge finished before Stop could be pressed, so this test measured nothing about " +
      "cancelling — it needs a slower merge, not a longer timeout",
  ).toBeVisible({ timeout: 5_000 });

  // While it runs the page says what is happening and does not animate a bar against nothing.
  await expect(page.locator(".working")).toContainText("not reportable while it runs");

  await stop.click();

  // Back to idle, with no half-result offered.
  await expect(page.getByRole("button", { name: "Merge", exact: true })).toBeVisible();
  await expect(page.locator(".result")).toHaveCount(0);

  // DURABLE, not "absent on the first poll". The stale reply from the discarded worker
  // arrives a microtask after `discard()` settles it, so an assertion that ran immediately
  // would pass whether or not the generation guard existed. Waiting for the announcement puts
  // a real round trip between the click and the check, and the second look is what makes the
  // absence mean something.
  await expect(page.locator('[role="status"]').last()).toHaveText("Stopped.");
  await expect(
    page.locator(".notice"),
    "a cancelled merge reported a failure — the person stopped it, nothing went wrong",
  ).toHaveCount(0);

  // AND THE BREAKER DID NOT LATCH. A cancel is a respawn, not a crash: counting respawns took
  // a page offline while every worker was healthy, which is why ADR 0015 §3 counts crashes.
  // The proof is that the very next merge works — on the SAME page, because the breaker's
  // state lives in that page's worker host and a reload would throw away the thing under test.
  const file = await mergeAndDownload(page);
  expect(file.suggestedFilename()).toBe("pages-137-merged.pdf");
  await expect(
    page.locator(".notice"),
    "the page reported EngineUnavailable after a cancel, so a respawn was counted as a crash",
  ).toHaveCount(0);
});

test("the whole flow works from the keyboard alone", async ({ page }) => {
  await page.goto("/merge-pdf");

  // Choosing a file is the one step a browser will not let a test drive from the keyboard —
  // the picker is the operating system's. What is asserted instead is that the control is
  // REACHABLE by keyboard and carries its own accessible name, which is the part this page
  // is responsible for; everything after it is done with keys and nothing else.
  // Tabbing from the top of the document, through the masthead, must ARRIVE at it. The bound
  // is what makes this an assertion rather than a loop: a control buried behind twenty stops
  // is reachable in the letter-of-the-law sense and unusable in practice.
  let stops = 0;
  let onTheInput = false;
  while (stops < 8 && !onTheInput) {
    await page.keyboard.press("Tab");
    stops += 1;
    onTheInput = await page.evaluate(() => {
      const active = document.activeElement;
      return active?.tagName === "INPUT" && active.getAttribute("type") === "file";
    });
  }
  expect(onTheInput, `the file input was not reachable within ${stops} tab stops`).toBe(true);

  await choose(page, ["pages-10.pdf", "pages-137.pdf"]);

  // Reorder with the keyboard: tab to the second file's "Move up" and press it.
  const moveUp = page.getByRole("button", { name: "Move pages-137.pdf up" });
  await moveUp.focus();
  await page.keyboard.press("Enter");
  await expect(listedNames(page)).toHaveText(["pages-137.pdf", "pages-10.pdf"]);

  // Merge with the keyboard.
  const merge = page.getByRole("button", { name: "Merge", exact: true });
  await merge.focus();
  await page.keyboard.press("Enter");

  // And take the result with the keyboard too: the link is focusable and Enter activates it,
  // which is the last step of the flow and the one a mouse-only implementation would break.
  const link = page.locator(".result a.download");
  await expect(link).toBeVisible({ timeout: 45_000 });
  await link.focus();
  await expect(link).toBeFocused();
  const download = page.waitForEvent("download");
  await page.keyboard.press("Enter");
  expect((await download).suggestedFilename()).toBe("pages-137-merged.pdf");

  // And every control the flow needs has a name a screen reader can read out. A button whose
  // only label is an arrow glyph would pass a click test and fail a person.
  for (const name of ["Move pages-10.pdf up", "Move pages-10.pdf down", "Remove pages-10.pdf"]) {
    await expect(page.getByRole("button", { name }), `no control named "${name}"`).toHaveCount(1);
  }
});

test("the tool page's console stays empty through a merge and through a refusal", async ({
  page,
}) => {
  // `console-silence.spec.ts` proves this thoroughly, against every failure path, on the
  // harness. This is the same assertion against the page that actually ships — a stray
  // `console.log` in the island, or a Svelte hydration warning, is invisible to that file.
  const noise: string[] = [];
  page.on("console", (message) => noise.push(`page console.${message.type()}: ${message.text()}`));
  page.on("pageerror", (error) => noise.push(`pageerror: ${error.message}`));

  await page.goto("/merge-pdf");
  await choose(page, ["pages-10.pdf", "not-a-pdf.bin", "encrypted.pdf"]);
  await page.getByRole("button", { name: "Remove not-a-pdf.bin" }).click();
  await page.getByRole("button", { name: "Remove encrypted.pdf" }).click();

  await mergeAndDownload(page);

  const ours = noise.filter((line) => !isBrowserPolicyReport(line));
  expect(ours, `the tool page must add nothing to the console:\n${ours.join("\n")}`).toEqual([]);
});

test("merging on the tool page makes no request beyond the pinned engine artifacts", async ({
  page,
}, testInfo) => {
  // The privacy claim in this page's own prose — "the files are not uploaded, because burrow
  // has no server that could receive them" — measured at the server, on the page that makes
  // it. The harness version of this test says nothing about a route a person can visit.
  await page.goto("/merge-pdf");
  // The first file is what STARTS the worker and fetches the engines, and those fetches are
  // legitimate. The marker goes after them, so everything counted below happens on a page
  // whose engines are already loaded — the same discipline `zero-requests.spec.ts` uses.
  await choose(page, ["pages-10.pdf"]);
  await mark(`merge-page:${testInfo.project.name}`);

  await choose(page, ["pages-137.pdf", "encrypted.pdf", "not-a-pdf.bin"], 1);
  await page.getByRole("button", { name: "Remove encrypted.pdf" }).click();
  await page.getByRole("button", { name: "Remove not-a-pdf.bin" }).click();
  await mergeAndDownload(page);

  const after = since(`merge-page:${testInfo.project.name}`);
  const unexpected = after.filter((entry) => !isPinnedArtifact(entry.url));
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the merge page uploaded something, or fetched something it did not need — the marker is " +
      "set after the engines are already loaded, so nothing at all should appear here",
  ).toEqual([]);
});
