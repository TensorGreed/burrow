// The render bundle loads in a real browser, and nothing loads it until something asks.
//
// ADR 0026 ships PDFium in a SECOND worker bundle. Two claims come with that, and they fail in
// opposite directions, so both are checked here rather than one being inferred from the other:
//
//   * **Nobody who is not rendering pays for it.** Checked over files on disk by
//     `tools/check-pdfium-is-render-only.sh` and as arithmetic by `src/size-budget.test.ts` --
//     and neither of those can see what a *browser* fetches. This file watches the test
//     server's own accept log while a page loads and an ordinary operation runs, which is the
//     only place that question is answered about behaviour rather than about bytes.
//   * **It works when something does ask.** A boundary with nothing behind it is a boundary no
//     test can drive, and an untested `blob:` worker whose fail-closed guard decides whether
//     file bytes may be touched is exactly the thing that goes wrong quietly.
//
// THE REQUEST LOG IS THE GROUND TRUTH, not `page.on("request")`. Browser-reported network
// events for dedicated workers are not equally complete across engines, and the worker is the
// only place file bytes ever exist. `e2e/server.mjs` logs every request it serves.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import { mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

function fixtureBytes(relative: string): number[] {
  return Array.from(readFileSync(join(conformance, relative)));
}

/** Requests for PDFium, by the shape of its staged name. */
function pdfiumRequests(urls: string[]): string[] {
  return urls.filter((url) => /pdfium|render-worker/.test(url));
}

test("a page that does not render fetches no part of the render bundle", async ({ page }) => {
  const marker = "render-worker/no-render";
  await mark(marker);

  // The harness loads the BASE bundle on arrival and runs an ordinary operation, which is what
  // a person on `/merge-pdf` does. `renderPageCount` is deliberately not called.
  await openHarness(page);
  const reply = await page.evaluate(() =>
    window.burrowHarness.run("page_count", Array.from([37, 80, 68, 70])),
  );
  // The call is expected to FAIL -- four bytes are not a PDF -- and that is fine: what matters
  // is that a real operation ran end to end through the base worker. A refusal exercises the
  // same load path a success does.
  expect(reply.kind, "the base worker must have answered at all").not.toBe("");

  // NOT BUILT, ASKED RATHER THAN INFERRED. A host that exists but has spawned nothing would
  // still fetch nothing, and this distinguishes "nobody asked" from "it failed quietly".
  expect(await page.evaluate(() => window.burrowHarness.renderState())).toBe("unbuilt");

  const requested = since(marker).map((entry) => entry.url);
  expect(
    pdfiumRequests(requested),
    "a page that renders nothing requested part of the render bundle",
  ).toEqual([]);

  // AND THE CONTROL, which is what stops the assertion above passing over an empty log. If the
  // page fetched nothing at all -- a server that stopped logging, a mark taken after the fact
  // -- there would be nothing for the filter to reject either.
  expect(
    requested.filter((url) => /qpdf\.[0-9a-f]{16}\.wasm/.test(url)).length,
    "the base engine was not fetched either, so this test watched nothing",
  ).toBeGreaterThan(0);
});

test("the render bundle loads on demand and answers through PDFium", async ({ page }) => {
  await openHarness(page);
  const marker = "render-worker/on-demand";
  await mark(marker);

  const reply = await page.evaluate(
    (bytes) => window.burrowHarness.renderPageCount("pages-137.pdf", new Uint8Array(bytes)),
    fixtureBytes("fixtures/pages-137.pdf"),
  );

  // The KIND, never the message. `Error::to_string()` can render an engine's object numbers
  // and byte offsets, and a Playwright failure message is a CI log like any other.
  expect(reply.ok, `unexpected ${reply.kind}`).toBe(true);
  // An awkward number on purpose: an off-by-one or a truncated count is visible in a way it
  // would not be for 1, 10, or a power of two.
  expect(reply.pages).toBe(137);

  // AND IT WAS REALLY FETCHED, at this point and not before. Without this the test would pass
  // against a build that had somehow loaded PDFium during start-up -- which is the failure the
  // first test in this file exists to catch, and a second assertion of it here costs nothing.
  const requested = since(marker).map((entry) => entry.url);
  expect(pdfiumRequests(requested).length, "the render bundle was never fetched").toBeGreaterThan(
    0,
  );
});

test("the render bundle is silent, including on a file it refuses", async ({ page }) => {
  // PDFIUM'S GLUE IS THE NOISY ONE, AND IT IS NOT BUILT WITH OUR FLAGS. `qpdf.js` is compiled
  // here with `-sENVIRONMENT=web,worker`; `pdfium.js` is a prebuilt unpacked verbatim, so it
  // carries every default path including the ones that write to the console. What silences it
  // is `...silent` in `render-prelude.js` -- three characters that are easy to lose and whose
  // absence nothing else would notice.
  //
  // The thorough version of this discipline, with canary fixtures and a control that
  // deliberately logs, is `e2e/console-silence.spec.ts` for the base bundle. This is the
  // second bundle's share of it: the operation that answers, and the one that refuses, since
  // a refusal is when an Emscripten module has the most to say.
  const noise: string[] = [];
  page.on("console", (message) => noise.push(message.text()));

  await openHarness(page);

  const good = await page.evaluate(
    (bytes) => window.burrowHarness.renderPageCount("pages-10.pdf", new Uint8Array(bytes)),
    fixtureBytes("fixtures/pages-10.pdf"),
  );
  expect(good.ok, `unexpected ${good.kind}`).toBe(true);

  const refused = await page.evaluate(() =>
    window.burrowHarness.renderPageCount("not-a-pdf.pdf", new Uint8Array([37, 80, 68, 70])),
  );
  expect(refused.ok, "four bytes are not a document; this should have been refused").toBe(false);

  // Object numbers, byte offsets, file paths -- never a blanket "the console is empty".
  //
  // THE CSP GUARD'S OWN PROBE IS IN SCOPE HERE, and an earlier version of this comment claimed
  // otherwise ("this page has already started one before the mark" -- there is no mark in this
  // test, and the listener is attached before `openHarness`). The guard proves a policy is in
  // force by making a request the policy must refuse, and Firefox and WebKit log that refusal
  // on every worker spawn (ADR 0015 §8). So the assertion is about what a FILE could put on
  // the console, which is what must never appear, rather than about silence -- and
  // `e2e/console-silence.spec.ts` is where the blanket version lives, with the probe excluded
  // by path and the excluded lines searched for the canary separately.
  expect(noise.join("\n")).not.toMatch(/WARNING|offset|object \d|\.pdf/i);
});
