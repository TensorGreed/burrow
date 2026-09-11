// Once the engines have loaded, nothing leaves the page. Measured at the server.
//
// THE OTHER HALF OF THE GUARANTEE
//
// ADR 0014 §3 states it in two lines, and both are load-bearing:
//
//   > No cross-origin requests, from the page or the worker — enforced by the browser.
//   > No requests at all after engine init — enforced by test.
//
// The second line exists because of a property of CSP that is impossible to work around:
// **CSP ignores query strings.** `…/qpdf.<hash>.wasm?leak=<bytes>` matches the permitted
// source and is allowed through. No policy that permits the engines to load can close that,
// because the permitted source is what is being abused.
//
// `e2e/csp.spec.ts` contains a test named
// "the query-string hole is real, and is why the zero-requests test exists", which
// **deliberately succeeds** at exactly that exfiltration. It is there so that two passing
// "blocked" tests are not mistaken for a complete guarantee. THIS file is the test it names.
// Neither is sufficient alone, and each should be read with the other.
//
// WHY THE SERVER'S LOG IS THE GROUND TRUTH
//
// Not `page.on("request")`. Browser-reported network events for **dedicated workers** are not
// equally complete across the three engines, and the worker is the only place file bytes ever
// exist — so the half that matters is precisely the half the browser reports least reliably.
// A server's accept log has no such gap: if a request reached it, it happened.
//
// The marker separating "during init" from "after init" is written to the log file by this
// process, directly. Asking the server to record it would mean making a request, and a request
// is the thing being counted.

import { readFileSync } from "node:fs";
import { appendFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import { LOG_PATH } from "./server.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

interface LogEntry {
  origin: string;
  method: string;
  url: string;
  at: number;
  agent: string;
  marker?: string;
}

function readLog(): LogEntry[] {
  return readFileSync(LOG_PATH, "utf8")
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as LogEntry);
}

/** Write a marker straight into the log. No request, so nothing is counted by writing it. */
async function mark(marker: string): Promise<void> {
  await appendFile(LOG_PATH, `${JSON.stringify({ marker, at: Date.now() })}\n`);
}

/** Everything logged after the last occurrence of `marker`. */
function since(marker: string): LogEntry[] {
  const entries = readLog();
  const at = entries.map((e) => e.marker).lastIndexOf(marker);
  expect(at, `the marker ${marker} was never written`).toBeGreaterThanOrEqual(0);
  return entries.slice(at + 1).filter((entry) => entry.marker === undefined);
}

function fixture(name: string): number[] {
  return Array.from(readFileSync(join(conformance, "fixtures", name)));
}

/**
 * Requests a **respawn** legitimately makes: the four pinned artifacts, and nothing else.
 *
 * A fresh worker re-fetches the engine modules and the bundle. That is not a leak and must not
 * be asserted away — but it must be recognised by EXACT path, so a request that merely looks
 * engine-ish, or an engine URL carrying a query string, is still caught. The query string is
 * the entire point of this file.
 */
function isPinnedArtifact(url: string): boolean {
  return (
    /^\/engines\/(pdfium|qpdf|burrow_wasm_bg)\.[0-9a-f]{16}\.wasm$/.test(url) ||
    /^\/engines\/burrow-worker\.[0-9a-f]{16}\.js$/.test(url) ||
    /^\/engines\/control\.[0-9a-f]{16}\.txt$/.test(url)
  );
}

test("no request of any kind is made while files are processed", async ({ page }, testInfo) => {
  await openHarness(page);
  await mark(`initialised:${testInfo.project.name}`);

  // The whole corpus, through both engines, plus the paths a respawn goes down — so that if a
  // respawn's own fetches were going to appear, they appear here where they can be recognised
  // rather than in some later run where they would be a mystery.
  const files = [
    "blank-1page.pdf",
    "pages-10.pdf",
    "pages-137.pdf",
    "truncated.pdf",
    "not-a-pdf.bin",
    "no-pages.pdf",
    "encrypted.pdf",
    "xref-bomb.pdf",
  ];

  for (const name of files) {
    for (const op of ["page_count", "structure_check"] as const) {
      await page.evaluate(
        ([operation, bytes]) =>
          window.burrowHarness.run(
            operation as "page_count" | "structure_check",
            bytes as number[],
          ),
        [op, fixture(name)] as const,
      );
    }
  }

  // A recycle and a respawn, deliberately included. Both fetch the engines again, and both are
  // exactly the moment a stray request would hide among legitimate ones.
  await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes, { limits: { maxMemoryBytes: 1024 } }),
    fixture("pages-10.pdf"),
  );
  await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes),
    fixture("pages-10.pdf"),
  );

  const after = since(`initialised:${testInfo.project.name}`);
  const unexpected = after.filter((entry) => !isPinnedArtifact(entry.url));

  expect(
    unexpected.map((entry) => `${entry.method} ${entry.origin}${entry.url}`),
    "nothing may be requested once the engines have loaded — a query string on a permitted " +
      "engine URL is the channel CSP cannot close, which is why this is asserted here",
  ).toEqual([]);

  // The complement. Without it, a server that had stopped logging entirely would pass every
  // assertion above — and so would a page that had silently stopped working.
  expect(
    after.length,
    "the respawns above must have re-fetched the engines; zero entries means the log is dead",
  ).toBeGreaterThan(0);
});

test("THE CONTROL: a deliberate exfiltration through the query-string hole IS seen", async ({
  page,
}, testInfo) => {
  // The assertion in the test above is "no unexpected entry appeared". A log that had silently
  // stopped recording would satisfy it perfectly, and so would a filter that accidentally
  // classified everything as a pinned artifact. This is what makes that assertion mean
  // something: the same channel an attacker would use, exercised on purpose, and asserted to
  // show up.
  //
  // It is deliberately the query-string hole and not some other request, because that is the
  // one the browser CANNOT stop — CSP ignores query strings, so `…/qpdf.<hash>.wasm?leak=…`
  // matches the permitted source. `e2e/csp.spec.ts` proves the browser allows it; this proves
  // the server log catches it. Together they are the whole of ADR 0014 §3.
  await openHarness(page);
  await mark(`control:${testInfo.project.name}`);

  const permitted = await page.evaluate(() => {
    const meta = document.querySelector('meta[http-equiv="Content-Security-Policy"]');
    const content = meta?.getAttribute("content") ?? "";
    const directive = content.split(";").find((d) => d.trim().startsWith("connect-src"));
    return directive?.trim().split(/\s+/)[1] ?? "";
  });
  expect(permitted).toMatch(/\/engines\/.*\.wasm$/);

  const secret = `exfiltrated-${testInfo.project.name}`;
  await page.evaluate(
    async ([url, payload]) => {
      await fetch(`${url}?leak=${payload}`, { mode: "no-cors", cache: "no-store" });
    },
    [permitted, secret] as const,
  );

  const seen = since(`control:${testInfo.project.name}`);
  const leaked = seen.filter((entry) => entry.url.includes(`leak=${secret}`));
  expect(
    leaked.length,
    "the deliberate leak did not reach the log, so the test above is measuring nothing",
  ).toBe(1);
  // And it must NOT be classified as a legitimate respawn fetch, or the filter in the test
  // above would wave it through.
  expect(
    isPinnedArtifact(leaked[0].url),
    "an engine URL with a query string must not read as a pinned artifact",
  ).toBe(false);
});

test("nothing at all reaches the second origin", async ({ page }, testInfo) => {
  // CSP should refuse a cross-origin request before it is ever sent, so this log should be
  // empty for a reason that has nothing to do with this test. Asserted anyway, because a
  // check that only confirms what CSP already promises is worth having exactly when CSP stops
  // delivering it — a missing meta tag, a misgenerated policy, or a worker that did not
  // inherit one. All three have happened in this project's history.
  await openHarness(page);
  await mark(`foreign:${testInfo.project.name}`);

  const foreignPort = Number(process.env.BURROW_FOREIGN_PORT ?? 4322);
  const foreign = `http://localhost:${foreignPort}`;

  // Try, from the page and from inside a worker, to reach it. Both must fail at the browser.
  const fromPage = await page.evaluate(async (url) => {
    try {
      await fetch(`${url}/collect`, { mode: "no-cors" });
      return "reached";
    } catch {
      return "refused";
    }
  }, foreign);
  expect(fromPage).toBe("refused");

  const fromWorker = await page.evaluate(
    (url) => window.burrowHarness.fetchFromWorker(`${url}/collect`),
    foreign,
  );
  expect(fromWorker.failed, "the probe worker failed to start").toBeFalsy();
  expect(fromWorker.blocked).toBe(true);

  const arrived = since(`foreign:${testInfo.project.name}`).filter((entry) =>
    entry.origin.endsWith(`:${foreignPort}`),
  );
  expect(
    arrived.map((entry) => `${entry.method} ${entry.url}`),
    "a request reached another origin — the policy is not in force where it was assumed to be",
  ).toEqual([]);
});
