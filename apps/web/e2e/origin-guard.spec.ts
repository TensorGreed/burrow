import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { ORIGIN } from "../playwright.config.js";

/**
 * The page says so when it is not where it was built to be.
 *
 * WHY THIS NEEDS A BROWSER RATHER THAN A FAKE DOCUMENT. The comparison is unit-tested in
 * `src/origin-guard.test.ts`; what cannot be tested there is the part that actually fails in
 * production — whether the guard's script RUNS at all under `script-src 'self'` with no
 * `'unsafe-inline'` and no nonce. An Astro `<script>` block is bundled to an external
 * `_astro/*.js` precisely so that it can, and every previous attempt to run code from a page
 * in this repo that got that wrong rendered as dead markup with nothing but a CSP line in a
 * console nobody was reading. A fake document cannot notice that.
 *
 * WHY ITS OWN SERVER. The mismatch has to be a real different origin, and the two servers
 * `playwright.config.ts` starts are the site (matching, by construction) and the foreign
 * logger (which serves nothing). This one serves the same `dist/` on a third port and writes
 * NOTHING to `e2e/server.mjs`'s request log — `zero-requests.spec.ts` treats that log as
 * ground truth and runs in a parallel worker, so a second writer would corrupt its evidence.
 */

const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, "..", "dist");

/** A port no other part of the suite uses. 4321 is the site, 4322 the foreign logger. */
const MISMATCH_PORT = 4323;
const MISMATCH_ORIGIN = `http://localhost:${MISMATCH_PORT}`;

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".woff2": "font/woff2",
  ".wasm": "application/wasm",
  ".txt": "text/plain; charset=utf-8",
};

let server: Server;

test.beforeAll(async () => {
  server = createServer((request, response) => {
    void (async () => {
      try {
        const path = decodeURIComponent((request.url ?? "/").split("?")[0].split("#")[0]);
        let file = join(dist, normalize(path));
        // The same containment check the real server makes. A test server that can be walked
        // out of is a test server that will be copied into something that matters.
        if (!file.startsWith(dist + sep) && file !== dist) {
          response.writeHead(403).end();
          return;
        }
        if ((await stat(file).catch(() => null))?.isDirectory()) {
          file = join(file, "index.html");
        }
        const info = await stat(file).catch(() => null);
        if (!info?.isFile()) {
          response.writeHead(404).end();
          return;
        }
        response.writeHead(200, {
          "content-type": TYPES[extname(file)] ?? "application/octet-stream",
        });
        createReadStream(file).pipe(response);
      } catch {
        response.writeHead(500).end();
      }
    })();
  });
  await new Promise<void>((ready) => server.listen(MISMATCH_PORT, "127.0.0.1", ready));
});

test.afterAll(async () => {
  await new Promise<void>((done) => server.close(() => done()));
});

// NOTE ON `_headers`: this server does not send it, so the only policy in force here is the
// `<meta>` CSP — which is exactly the case ADR 0014 §5 duplicates it for, a host that ignores
// the file. The guard must work under that, because a misconfigured deploy is precisely when
// the weaker of the two policies is the one in force.

test("a page served from the wrong origin says so, in the page, before anything is touched", async ({
  page,
}) => {
  await page.goto(`${MISMATCH_ORIGIN}/split-pdf/`);

  const banner = page.locator(".origin-mismatch");
  await expect(banner).toBeVisible();
  await expect(banner).toHaveAttribute("role", "alert");

  const text = (await banner.textContent()) ?? "";
  // BOTH origins and the action. A message naming one leaves the reader guessing which half
  // is wrong, and this is read by somebody who has just deployed and found a dead page.
  expect(text).toContain(ORIGIN);
  expect(text).toContain(MISMATCH_ORIGIN);
  expect(text).toContain(`BURROW_SITE=${MISMATCH_ORIGIN}`);
  expect(text).toMatch(/nothing has been sent anywhere/i);

  // FIRST IN THE BODY, so it is first in reading order and the first thing a screen reader
  // reaches — not appended below a tool that will not work.
  const firstClass = await page.evaluate(() => document.body.firstElementChild?.className ?? "");
  expect(firstClass).toBe("origin-mismatch");
});

test("every route carries the stamp, so the guard has something to compare on all of them", async ({
  page,
}) => {
  // A per-route assertion because the stamp lives in the shared layout, and "the shared
  // layout" is a claim rather than a fact until each route is looked at. A page that opted
  // out would be a page where this guard silently returns `unknown`.
  for (const route of [
    "/",
    "/merge-pdf/",
    "/rotate-pdf/",
    "/reorder-pdf/",
    "/split-pdf/",
    "/credits/",
  ]) {
    await page.goto(`${MISMATCH_ORIGIN}${route}`);
    const builtFor = await page.getAttribute('meta[name="burrow-built-for"]', "content");
    expect(builtFor, `${route} carries no build origin`).toBe(ORIGIN);
    await expect(
      page.locator(".origin-mismatch"),
      `${route} did not report the mismatch`,
    ).toBeVisible();
  }
});

test("THE CONTROL: the right origin gets no banner at all", async ({ page }) => {
  // Without this, every assertion above would also pass against a guard that shows the banner
  // unconditionally — which would put a false refusal on every correctly-deployed page and is
  // the worse failure of the two.
  await page.goto("/split-pdf/");
  await expect(page.locator(".origin-mismatch")).toHaveCount(0);
  expect(await page.getAttribute('meta[name="burrow-built-for"]', "content")).toBe(ORIGIN);
});
