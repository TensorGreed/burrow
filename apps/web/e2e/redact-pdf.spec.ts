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
import { inflateSync } from "node:zlib";
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

/**
 * A committed fixture whose whole first page redacts (`outcomes.tsv`: `whole OK`). The untagged
 * LibreOffice export: #125 slice 3 refuses the tagged original `[structure-tree]`.
 */
const REDACTS = "producer-writer-untagged.pdf";
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
  await expect(link).toHaveText(/producer-writer-untagged-redacted\.pdf/);

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

test("redacting makes no request beyond redaction's and the picture's pinned engine artifacts", async ({
  page,
}, testInfo) => {
  // THE MARKER GOES BEFORE `goto`, so start-up is in the window too: the page must never reach
  // for the base worker, and a respawn re-fetches only redaction's set. Since S4 the picture is
  // drawn by the render bundle (D1), which is admitted by name and required below.
  await mark(`redact-page:${testInfo.project.name}`);
  const frame = await open(page, REDACTS);
  await expect(page.getByTestId("redact-picture")).toHaveAttribute("data-state", "drawn", {
    timeout: 45_000,
  });
  await area(page, "0", "0", String(frame.width), String(frame.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });

  const after = since(`redact-page:${testInfo.project.name}`);
  // EVERY REQUEST IS ACCOUNTED FOR (round 2's C): redaction's pinned engine artifacts, or one of
  // the page's own static files by exact shape. Anything else on the origin fails, by name.
  // ON THIS SITE'S ORIGIN, and only then by path: a page asset or an engine artifact fetched from
  // anywhere else is no artifact of this page's.
  const site = new URL(page.url()).origin;
  const unexpected = after.filter(
    (entry) =>
      entry.origin !== site ||
      (!isRedactionArtifact(entry.url, "with-render") && !isPageAsset(entry.url)),
  );
  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "the redaction page requested something that is neither its engine nor its own static files",
  ).toEqual([]);
  // THE CONTROL: redaction's own module was fetched, so the window watched something.
  expect(
    after.filter((entry) => /burrow_wasm_redact_bg/.test(entry.url)).length,
    "redaction's module was not fetched, so this test watched nothing",
  ).toBeGreaterThan(0);
  // AND THE PICTURE'S: the render worker drew it, so the admission above was used.
  expect(
    after.filter((entry) => /burrow-render-worker/.test(entry.url)).length,
    "the render worker was not fetched, so the picture was drawn by something else",
  ).toBeGreaterThan(0);
  expect(
    after.filter((entry) => entry.url === "/redact-pdf/").length,
    "the page itself was not logged, so the allow-list below matched nothing",
  ).toBeGreaterThan(0);
  // AND NOTHING BUT GETs of the page's own static files: no upload of any kind.
  const posted = after.filter((entry) => entry.method !== "GET");
  expect(posted.map((entry) => `${entry.method} ${entry.url}`)).toEqual([]);
});

/**
 * The held page's own static files, by exact shape: the page, its content-hashed Astro chunks and
 * stylesheets, and the self-hosted font. Not a prefix match on the origin: a path the page has no
 * reason to fetch fails the test above whatever directory it is in.
 */
function isPageAsset(url: string): boolean {
  return (
    url === "/redact-pdf/" ||
    // FIREFOX ASKS FOR THIS ITSELF, for the tab's icon; the page names no icon and sends nothing.
    url === "/favicon.ico" ||
    /^\/_astro\/[A-Za-z0-9_.-]+\.(js|css)$/.test(url) ||
    /^\/fonts\/[a-z0-9.-]+\.woff2$/.test(url)
  );
}

// ---- S6b: another area of the result --------------------------------------------------------

/** A generated fixture with two lines of literal text: `BURROW-SECRET-01` above `KEEP-THIS-LINE`. */
const TWO_LINES = "01-plain-tj.pdf";
/** The first line, in the 400 x 200 page's display frame. */
const FIRST_LINE = ["30", "70", "330", "40"] as const;
/** The second line. */
const SECOND_LINE = ["30", "150", "330", "30"] as const;

/**
 * Whether `token` is in a document's bytes, read WITHOUT burrow: the raw bytes, and every stream
 * inflated where it inflates. A text-showing string is literal in this fixture, so a token still
 * drawn is found; one removed is not.
 */
function holds(bytes: Buffer, token: string): boolean {
  if (bytes.includes(token)) return true;
  const text = bytes.toString("latin1");
  for (const m of text.matchAll(/stream\r?\n/g)) {
    const start = (m.index ?? 0) + m[0].length;
    const end = text.indexOf("endstream", start);
    if (end < 0) continue;
    try {
      if (inflateSync(bytes.subarray(start, end)).includes(token)) return true;
    } catch {
      // Not deflated, or not whole: the raw search above covered it.
    }
  }
  return false;
}

async function downloaded(page: Page, testId: string): Promise<Buffer> {
  const download = page.waitForEvent("download");
  await page.getByTestId(testId).click();
  const stream = await (await download).createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

/**
 * Whether the browser still serves an object URL, asked the way a person asks: by downloading it.
 *
 * NOT `fetch`: the page's CSP `connect-src` names only the engine URLs, so a fetch of a `blob:`
 * URL fails whether or not it was revoked, and a probe built on it reports "released" for both.
 * A download of a revoked URL fails, and Playwright says so; one that never starts is a failure.
 */
async function served(page: Page, url: string): Promise<boolean> {
  const download = page.waitForEvent("download", { timeout: 10_000 }).catch(() => null);
  await page.evaluate((href) => {
    const link = document.createElement("a");
    link.href = href;
    link.download = "probe.pdf";
    document.body.append(link);
    link.click();
    link.remove();
  }, url);
  const started = await download;
  if (started === null) return false;
  return (await started.failure()) === null;
}

test("another area is redacted from the result, and the first removal stays removed (S6b)", async ({
  page,
}) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  const first = await downloaded(page, "redact-download");
  // THE EXTRACTOR'S CONTROL: it reads the line that was kept, so its silence below means something.
  expect(holds(first, "KEEP-THIS-LINE"), "the extractor does not read this fixture").toBe(true);
  expect(holds(first, "BURROW-SECRET-01"), "the first area was not removed").toBe(false);

  await page.getByTestId("redact-another").click();
  await expect(page.locator(".chosen__name")).toHaveText("01-plain-tj-redacted.pdf");
  await expect(page.getByTestId("redact-frame")).toBeVisible({ timeout: 45_000 });
  await expect(page.getByTestId("redact-left")).toHaveValue("");
  await area(page, ...SECOND_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await expect(page.getByTestId("redact-download")).toHaveText(/01-plain-tj-redacted\.pdf$/);
  const second = await downloaded(page, "redact-download");
  expect(holds(second, "KEEP-THIS-LINE"), "the second area was not removed").toBe(false);
  // THE PROPERTY: the second run was on the first one's output, so its removal is still there.
  expect(holds(second, "BURROW-SECRET-01"), "the second run undid the first removal").toBe(false);
});

test("the earlier result stays downloadable until the next run succeeds, then is released", async ({
  page,
}) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await page.getByTestId("redact-another").click();

  const earlier = page.getByTestId("redact-earlier").locator("a");
  await expect(earlier).toBeVisible({ timeout: 45_000 });
  const url = String(await earlier.getAttribute("href"));
  expect(await served(page, url), "the earlier result is not downloadable").toBe(true);

  await expect(page.getByTestId("redact-frame")).toBeVisible({ timeout: 45_000 });
  await area(page, ...SECOND_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await expect(page.getByTestId("redact-earlier")).toHaveCount(0);
  expect(await served(page, url), "the earlier result outlived the run that replaced it").toBe(
    false,
  );
});

test("choosing a different file releases the earlier result", async ({ page }) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await page.getByTestId("redact-another").click();
  const url = String(await page.getByTestId("redact-earlier").locator("a").getAttribute("href"));
  expect(await served(page, url)).toBe(true);

  await page.getByTestId("redact-file").setInputFiles(join(fixtures, REDACTS));
  await expect(page.getByTestId("redact-earlier")).toHaveCount(0);
  expect(await served(page, url), "a different file left the earlier result alive").toBe(false);
});

test("leaving the page releases every result it holds", async ({ page }) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await page.getByTestId("redact-another").click();
  const earlierUrl = String(
    await page.getByTestId("redact-earlier").locator("a").getAttribute("href"),
  );
  // AND A CURRENT RESULT: the second run's, held while the page is left.
  await expect(page.getByTestId("redact-frame")).toBeVisible({ timeout: 45_000 });
  await area(page, ...SECOND_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  const currentUrl = String(await page.getByTestId("redact-download").getAttribute("href"));
  expect(await served(page, currentUrl), "the current result is not downloadable").toBe(true);
  // THE EARLIER RESULT WENT WITH THE RUN THAT REPLACED IT, before the page was left: asserted
  // here, where it can fail, rather than after pagehide, where it is already true.
  expect(
    await served(page, earlierUrl),
    "the earlier result outlived the run that replaced it",
  ).toBe(false);

  // `pagehide` is what a navigation or a bfcache entry fires; the page releases on it.
  await page.evaluate(() => globalThis.dispatchEvent(new PageTransitionEvent("pagehide")));
  expect(await served(page, currentUrl), "leaving the page left the current result alive").toBe(
    false,
  );
});

test("leaving the page holding only an earlier result releases it", async ({ page }) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await page.getByTestId("redact-another").click();
  const url = String(await page.getByTestId("redact-earlier").locator("a").getAttribute("href"));
  expect(await served(page, url)).toBe(true);
  await page.evaluate(() => globalThis.dispatchEvent(new PageTransitionEvent("pagehide")));
  expect(await served(page, url), "leaving the page left a result alive").toBe(false);
});

test("a page restored from the back-forward cache mid-run is idle, not working", async ({
  page,
}) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  // RUN, LEAVE AND COME BACK IN ONE TASK, so the run is still in flight when the page is left.
  const stillWorking = await page.evaluate(async () => {
    (document.querySelector('[data-testid="redact-run"]') as HTMLButtonElement).click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const working = document.querySelector('[data-testid="redact-stop"]') !== null;
    globalThis.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true }));
    globalThis.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    return working;
  });
  expect(stillWorking, "the run had already finished when the page was left").toBe(true);
  await expect(page.getByTestId("redact-stop")).toHaveCount(0);
  await expect(page.getByTestId("redact-run")).toBeEnabled();
  await expect(page.getByTestId("redact-download")).toHaveCount(0);
});

test("a page restored mid-run discards the worker the run was on", async ({ page }, testInfo) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  // HOLD THE RUN IN FLIGHT: redaction's worker recycles after every reply, so the run spawns a
  // fresh worker, and that worker's fetch of redaction's module is held -- the run cannot finish
  // until the test lets it.
  // THE FIRST FETCH ONLY, held for the whole test: a later worker's fetch goes through, so the run
  // after the restore can finish if, and only if, it is on a new worker. (Unrouting released the
  // held request too, which let the old worker finish and passed the test with the discard
  // removed -- caught by its own mutation.)
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => (release = resolve));
  const engine = "**/engines/burrow_wasm_redact_bg.*.wasm";
  let seen = 0;
  await page.route(engine, async (route) => {
    seen += 1;
    if (seen === 1) await held;
    // The worker the held request came from is terminated by the restore, which aborts it, and
    // continuing an aborted request is refused: the expected outcome, not a failure.
    await route.continue().catch(() => {});
  });
  const fetching = page.waitForRequest(/burrow_wasm_redact_bg\.[0-9a-f]{16}\.wasm/, {
    timeout: 15_000,
  });
  await page.getByTestId("redact-run").click();
  await fetching;
  await expect(page.getByTestId("redact-stop")).toBeVisible();

  // LEFT AND RESTORED from the back-forward cache, with the run still held.
  await page.evaluate(() => {
    globalThis.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true }));
    globalThis.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
  });
  await expect(page.getByTestId("redact-stop")).toHaveCount(0);

  // THE NEXT RUN IS ON A FRESH WORKER, which fetches redaction's set again and finishes. Had the
  // restore kept the held worker, this run would queue behind a run that never ends.
  await mark(`redact-restored:${testInfo.project.name}`);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 15_000 });
  const fresh = since(`redact-restored:${testInfo.project.name}`).filter((entry) =>
    /burrow_wasm_redact_bg/.test(entry.url),
  );
  expect(fresh.length, "the run after the restore did not start a fresh worker").toBeGreaterThan(0);
  release();
  await page.unroute(engine);
});

test("after a stopped second run, the earlier link says what it holds", async ({ page }) => {
  await open(page, TWO_LINES, generated);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await page.getByTestId("redact-another").click();
  await expect(page.getByTestId("redact-frame")).toBeVisible({ timeout: 45_000 });
  await area(page, ...SECOND_LINE);
  const stopped = await page.evaluate(async () => {
    (document.querySelector('[data-testid="redact-run"]') as HTMLButtonElement).click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const stop = document.querySelector<HTMLButtonElement>('[data-testid="redact-stop"]');
    stop?.click();
    return stop !== null;
  });
  expect(stopped).toBe(true);
  const earlier = page.getByTestId("redact-earlier").locator("a");
  await expect(earlier).toHaveText("Download the last result, with only the earlier areas removed");
  await expect(earlier).not.toContainText("01-plain-tj-redacted.pdf");
});

test("a §7 note stays through the chain and goes with a genuinely new file (L2)", async ({
  page,
}) => {
  // 09-actualtext drops one passage's carried text when its whole page is redacted; the second
  // area, a corner with nothing in it, counts nothing -- and the note must stay, because the first
  // removal, and what it cost, is in the second result too.
  const frame = await open(page, "09-actualtext.pdf", generated);
  await area(page, "0", "0", String(frame.width), String(frame.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  const notes = page.getByTestId("redact-result-note");
  await expect(notes).toHaveCount(1);

  await page.getByTestId("redact-another").click();
  await expect(page.getByTestId("redact-frame")).toBeVisible({ timeout: 45_000 });
  await area(page, "0", "0", "5", "5");
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await expect(notes).toHaveCount(1);
  await expect(notes).toContainText("had alternative text");

  const plain = await open(page, REDACTS);
  await area(page, "0", "0", String(plain.width), String(plain.height));
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await expect(notes).toHaveCount(0);
});

// ---- S4: the selection picture ---------------------------------------------------------------

/** The picture, drawn: its canvas's box on screen. */
async function drawnPicture(page: Page) {
  await expect(page.getByTestId("redact-picture")).toHaveAttribute("data-state", "drawn", {
    timeout: 45_000,
  });
  // IN VIEW FIRST: a pointer at a place off the screen reaches nothing.
  const canvas = page.getByTestId("redact-picture-canvas");
  await canvas.scrollIntoViewIfNeeded();
  const box = await canvas.boundingBox();
  expect(box, "the picture has no box on screen").not.toBeNull();
  return box!;
}

/**
 * How many render workers the page holds, found by what only the render bundle defines (as
 * `page-thumbnails.spec.ts` finds one): the PDFium bridge.
 */
async function renderWorkers(page: Page): Promise<number> {
  let count = 0;
  for (const worker of page.workers()) {
    const isRender = await worker
      .evaluate(
        () =>
          typeof (self as unknown as Record<string, unknown>)["__burrow_pdfium_copy_out"] ===
          "function",
      )
      .catch(() => false);
    if (isRender) count += 1;
  }
  return count;
}

/** Drag across the picture from one fraction of it to another. */
async function drag(
  page: Page,
  box: { x: number; y: number; width: number; height: number },
  from: [number, number],
  to: [number, number],
) {
  await page.mouse.move(box.x + from[0] * box.width, box.y + from[1] * box.height);
  await page.mouse.down();
  await page.mouse.move(box.x + to[0] * box.width, box.y + to[1] * box.height, { steps: 5 });
  await page.mouse.up();
}

async function boxes(page: Page): Promise<number[]> {
  return Promise.all(
    ["redact-left", "redact-top", "redact-width", "redact-height"].map(async (id) =>
      Number(await page.getByTestId(id).inputValue()),
    ),
  );
}

test("a drag on the picture fills the four boxes, and that area is what the run removes (S4)", async ({
  page,
}) => {
  // 01-plain-tj is 400 x 200 points: the secret line inside [30, 70, 330, 40], the kept line below.
  await open(page, TWO_LINES, generated);
  const box = await drawnPicture(page);
  await drag(page, box, [30 / 400, 70 / 200], [360 / 400, 110 / 200]);
  const [left, top, width, height] = await boxes(page);
  // A pointer lands on a CSS pixel, so the boxes are near the corners dragged, never inside them:
  // the drag is rounded outward to the tenth of a point.
  for (const [got, want] of [
    [left, 30],
    [top, 70],
    [left! + width!, 360],
    [top! + height!, 110],
  ] as const) {
    expect(Math.abs(got - want), `${got} is not near ${want}`).toBeLessThan(3);
  }
  await expect(page.getByTestId("redact-picture-outline")).toBeVisible();

  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  const out = await downloaded(page, "redact-download");
  expect(holds(out, "KEEP-THIS-LINE"), "the extractor does not read this fixture").toBe(true);
  expect(holds(out, "BURROW-SECRET-01"), "the dragged area was not the one removed").toBe(false);
});

test("the picture is usable by keyboard alone (S4)", async ({ page }) => {
  await open(page, TWO_LINES, generated);
  await drawnPicture(page);
  const surface = page.getByTestId("redact-picture-surface");
  await surface.focus();
  await page.keyboard.press("Enter");
  expect(await boxes(page)).toEqual([0, 0, 72, 36]);
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Alt+ArrowDown");
  await page.keyboard.press("Shift+ArrowRight");
  expect(await boxes(page)).toEqual([10, 1, 82, 36]);
  await expect(page.getByTestId("redact-picture-outline")).toBeVisible();
  // AND TYPING IS THE SAME SOURCE: a box typed moves the outline the keys draw from.
  await page.getByTestId("redact-left").fill("100");
  await surface.focus();
  await page.keyboard.press("ArrowLeft");
  expect((await boxes(page))[0]).toBe(90);
});

test("the picture draws the page's annotations, and an area over one refuses (S4, D3)", async ({
  page,
}) => {
  // 11-annotation's only ink in [30, 68, 340, 44] (display frame) is a FreeText annotation's
  // appearance; the page's own text is lower down. A picture without annotations is white there.
  await open(page, "11-annotation.pdf", generated);
  const box = await drawnPicture(page);
  const dark = await page.getByTestId("redact-picture-canvas").evaluate((node) => {
    const canvas = node as HTMLCanvasElement;
    const context = canvas.getContext("2d", { willReadFrequently: true })!;
    const sx = canvas.width / 400;
    const sy = canvas.height / 200;
    const { data } = context.getImageData(30 * sx, 68 * sy, 340 * sx, 44 * sy);
    let count = 0;
    for (let i = 0; i < data.length; i += 4) if (data[i]! < 128) count += 1;
    return count;
  });
  expect(dark, "the annotation was not drawn in the picture").toBeGreaterThan(50);

  // D3's condition: an area over a drawn annotation is refused, never kept or removed silently.
  await drag(page, box, [40 / 400, 75 / 200], [200 / 400, 105 / 200]);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-notice")).toContainText(CATEGORIES.annotations.title, {
    timeout: 45_000,
  });
  await expect(page.getByTestId("redact-notice")).toContainText("annotation-in-region");
});

test("the render worker is gone before a run and does not come back for it (S4, D1)", async ({
  page,
}) => {
  await open(page, TWO_LINES, generated);
  await drawnPicture(page);
  // THE CONTROL: the picture was drawn by a render worker this page holds.
  expect(await renderWorkers(page), "no render worker drew the picture").toBeGreaterThan(0);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  // GONE BY THE TIME THE RUN IS UNDER WAY: terminated when it began, the picture being idle.
  await expect.poll(() => renderWorkers(page), { timeout: 10_000 }).toBe(0);
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  expect(await renderWorkers(page), "a render worker came back for the run").toBe(0);
  // The picture already drawn stays: it is pixels on the page, not the engine.
  await expect(page.getByTestId("redact-picture")).toHaveAttribute("data-state", "drawn");
});

test("a re-fed result gets no picture (S4, D9)", async ({ page }) => {
  await open(page, TWO_LINES, generated);
  await drawnPicture(page);
  await area(page, ...FIRST_LINE);
  await page.getByTestId("redact-run").click();
  await expect(page.getByTestId("redact-download")).toBeVisible({ timeout: 45_000 });
  await page.getByTestId("redact-another").click();
  await expect(page.getByTestId("redact-frame")).toBeVisible({ timeout: 45_000 });
  await expect(page.getByTestId("redact-picture")).toHaveAttribute("data-state", "idle");
  await expect(page.getByTestId("redact-picture")).toContainText("no picture of a result");
  await expect(page.getByTestId("redact-picture-canvas")).toHaveCount(0);
});

test("at 390 px wide the page and its picture fit, with no sideways scroll (S4)", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await open(page, REDACTS);
  const box = await drawnPicture(page);
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow, "the page scrolls sideways at 390 px").toBeLessThanOrEqual(0);
  // And a drag still chooses an area there.
  await drag(page, box, [0.1, 0.1], [0.5, 0.3]);
  expect((await boxes(page))[2]).toBeGreaterThan(0);
});

test("nothing below the picture moves when it arrives (S4)", async ({ page }) => {
  // THE RENDER BUNDLE IS HELD BACK, so the picture is seen still drawing: its place must already
  // be reserved then. Measured without this: a click on the run button landed elsewhere when the
  // picture appeared under the pointer.
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route(/\/engines\/burrow-render-worker\./, async (route) => {
    await held;
    await route.continue();
  });
  await open(page, TWO_LINES, generated);
  await expect(page.getByTestId("redact-picture")).toHaveAttribute("data-state", "drawing");
  const before = await page.getByTestId("redact-run").boundingBox();
  release();
  await drawnPicture(page);
  await page.evaluate(() => window.scrollTo(0, 0));
  const after = await page.getByTestId("redact-run").boundingBox();
  expect(before).not.toBeNull();
  expect(after!.y - before!.y, "the run button moved when the picture arrived").toBe(0);
});

test("leaving the page lets go of the picture and ends its worker (S4, D1, D4)", async ({
  page,
}) => {
  await open(page, TWO_LINES, generated);
  await drawnPicture(page);
  expect(await renderWorkers(page), "no render worker drew the picture").toBeGreaterThan(0);
  await page.evaluate(() => globalThis.dispatchEvent(new PageTransitionEvent("pagehide")));
  await expect.poll(() => renderWorkers(page), { timeout: 10_000 }).toBe(0);
  // THE PIXELS WENT WITH IT: the picture waits to be drawn again, on a page shown again.
  await expect(page.getByTestId("redact-picture")).toHaveAttribute("data-state", "drawing");
  const size = await page
    .getByTestId("redact-picture-canvas")
    .evaluate((node) => (node as HTMLCanvasElement).width);
  expect(size, "the canvas kept the picture's pixels").toBe(0);
  await page.evaluate(() =>
    globalThis.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })),
  );
  await drawnPicture(page);
});
