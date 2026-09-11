// Nothing from a user's file reaches any console, on any failure path, in any browser.
//
// ROADMAP item 7's web half, and the thorough version `e2e/engines.spec.ts` explicitly defers
// to: every failure path, canary fixtures, every worker's console as well as the page's, and a
// control that deliberately logs.
//
// WHY THE ASSERTION IS "NOTHING", NOT "NO CANARY"
//
// `core/burrow-engines/tests/secret_leak.rs` established this by mutation. With the
// suppression removed, qpdf writes
//
//     WARNING: input (offset 4242): xref not found
//
// which contains no canary and is unmistakably derived from the file — a byte offset is file
// content. So the property asserted is the stronger and simpler one: an engine writes NOTHING.
// Anything at all is a failure, whether or not we recognise it.
//
// WHY THE WORKER'S CONSOLE IS CAPTURED IN THE WORKER
//
// Playwright does not deliver dedicated-worker console messages uniformly across the three
// browsers, and the worker is the only place file bytes ever exist — so the surface that
// matters most is the one the harness reports least reliably. The test-only prologue replaces
// `console.*` inside the worker and posts what it sees back to the page. Being the console is
// the only way to see every call in every engine.
//
// THE TWO LAYERS ARE EACH SUFFICIENT, AND THAT IS MEASURED
//
// A mutation sweep in M1 PR 4a-ii removed each layer on its own and this file stayed green:
//
//   * `printErr`/`print` un-stubbed on both Emscripten modules, C++ suppression intact — silent.
//   * `qpdf_silence_errors`, `qpdf_set_suppress_warnings` and the discarding logger all
//     removed, `printErr` still stubbed — silent.
//   * BOTH removed — twelve lines, including
//     `WARNING: input (offset 9): xref not found` and
//     `WARNING: input, object 3 0 at offset 131: kid 0 (from 0) Resources is missing or
//     invalid; repairing`.
//
// So ADR 0006 requirement 2's "at both layers" is genuine defence in depth rather than one
// mechanism with a spare. It also means this test cannot tell you WHICH layer regressed — only
// that one of them is the last one standing. That is the right trade (either layer alone keeps
// the guarantee), and it is stated here so nobody concludes from a green run that both work.
//
// WHY THE CONTROL MATTERS MORE THAN THE ASSERTIONS
//
// A leak test that cannot fail is worse than no leak test: it converts "nobody checked" into
// "something checked and it was fine". The last case here makes the worker log deliberately
// and asserts the capture SEES it. Without that, a capture broken by a refactor would leave
// every case above green.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { CANARY_PATH } from "./global-setup.mjs";
import { openHarness } from "./harness";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const conformance = join(repo, "tests", "conformance");

/**
 * The canary fixture and its mark, generated once per run by `globalSetup`.
 *
 * It comes from the SAME Rust generator the native leak test uses —
 * `core/burrow-engines/testsupport/minimal_pdf.rs`, which is included by `#[path]` in two
 * places rather than copied, "so there is one generator rather than two that drift". A
 * TypeScript reimplementation here would be the third copy and the first to drift, and the
 * whole value of this fixture is the placement of the canary: a name object, a string, a
 * stream's contents, a dictionary key, and a broken token immediately beside it so the parser
 * fails at exactly that offset.
 *
 * The mark is unique per run, so a stale match cannot be mistaken for a fresh leak.
 */
function loadCanary(): { mark: string; bytes: number[] } {
  return JSON.parse(readFileSync(CANARY_PATH, "utf8")) as { mark: string; bytes: number[] };
}

function fixture(name: string): number[] {
  return Array.from(readFileSync(join(conformance, "fixtures", name)));
}

/**
 * The one thing the browser emits that is not ours, and cannot be suppressed.
 *
 * **Firefox reports the fail-closed guard's own probe to the page console**, once per worker
 * spawn, measured in M1 PR 4a-ii:
 *
 *     Content-Security-Policy: The page's settings blocked the loading of a resource
 *     (connect-src) at http://localhost:4321/__csp-probe because it violates the following
 *     directive: "connect-src …"
 *
 * **WebKit does the same, in different words** — measured in the same run:
 *
 *     Refused to connect to http://localhost:4321/__csp-probe because it does not appear in
 *     the connect-src directive of the Content Security Policy.
 *
 * Note the unhyphenated spelling. The first version of this filter matched only Firefox's
 * "Content-Security-Policy" and WebKit failed, which is a reminder that the recognisable part
 * of these messages is the **probe path**, not the prose. Chromium does not surface either
 * from a worker; WebKit separately does not dispatch the `securitypolicyviolation` *event*
 * (ADR 0014 §1b), which is a different thing from not logging.
 *
 * There is no way to avoid any of it: the guard establishes that a policy is in force by
 * making a request the policy MUST refuse, and a browser logs refusals.
 *
 * It is excluded here, and the exclusion is deliberately narrow rather than a category:
 *
 *   * it is written by the browser, not by burrow, not by an engine, and not by wasm;
 *   * it names the probe path and the policy's own directive — both build constants, both
 *     already public in the page's `<meta>` tag;
 *   * it contains nothing derived from any file, which the canary assertion below re-checks
 *     against the excluded lines specifically rather than trusting this reasoning.
 *
 * ADR 0014 §5 rejected putting `frame-ancestors` in the meta tag because a per-page-load CSP
 * console error "would be noise that teaches a reader to ignore CSP console errors". This is
 * the same noise arriving from a different direction, and the trade is different: the guard is
 * what stops an unpoliced worker touching a file, which is worth one line in a console Firefox
 * users will see on every spawn. Recorded in ADR 0015 as an accepted cost, not overlooked.
 */
function isBrowserPolicyReport(line: string): boolean {
  // The PROBE PATH is what identifies it, and it is matched exactly. The prose differs per
  // browser and is matched loosely enough to cover both spellings, but a message that does not
  // name the guard's own probe URL is not one of these and is not excluded — which keeps the
  // exclusion from becoming "anything mentioning CSP".
  return /content.security.policy/i.test(line) && line.includes("/__csp-probe");
}

/** Everything that could carry a byte out of the page, watched at once. */
function watchEverything(page: Page) {
  const noise: string[] = [];
  page.on("console", (message) => noise.push(`page console.${message.type()}: ${message.text()}`));
  page.on("pageerror", (error) => noise.push(`pageerror: ${error.message}`));
  // NOT `page.on("worker")`. Playwright's `Worker` object exposes no console event, so a
  // listener there would collect nothing — an earlier version had one, and it read as evidence
  // while contributing none. What the worker writes is captured INSIDE the worker, by the
  // prologue, and merged with this list by the caller.
  return noise;
}

test("no failure path writes anything to any console", async ({ page }, testInfo) => {
  const { mark, bytes: canaryBytes } = loadCanary();
  const noise = watchEverything(page);

  // WAIT FOR THE FIRST WORKER before arming, then discard and build an instrumented one.
  //
  // `page.goto` alone is not enough: the driver starts its own `ready()` on load, so arming
  // immediately races that worker's engine fetches. Discarding mid-fetch made Firefox log an
  // aborted-request error intermittently — a flake produced entirely by this test, in the one
  // test whose whole job is to notice console output.
  //
  // The capture still covers a full initialisation, because the worker it watches is the
  // SECOND one and is instrumented from its first line.
  await openHarness(page);
  await page.evaluate(() => {
    window.burrowHarness.arm({ captureConsole: true });
    window.burrowHarness.discardWorker();
  });
  expect(await page.evaluate(() => window.burrowHarness.ready())).toBe(true);

  // EVERY failure path the page can reach, through both engines.
  const cases: {
    what: string;
    op: "page_count" | "structure_check";
    bytes: number[];
    // Per case, not for the whole loop. qpdf's repair path emits a DIFFERENT set of warnings
    // from its parse path — "Attempting to reconstruct cross-reference table" appears only
    // with recovery on — so a loop that passed `true` for everything tested one path twice and
    // called it two cases.
    attemptRecovery?: boolean;
    limits?: Record<string, number>;
  }[] = [
    { what: "the canary file through pdfium", op: "page_count", bytes: canaryBytes },
    {
      what: "the canary file through qpdf",
      op: "structure_check",
      bytes: canaryBytes,
      attemptRecovery: false,
    },
    {
      what: "the canary file through qpdf's repair path",
      op: "structure_check",
      bytes: canaryBytes,
      attemptRecovery: true,
    },
    { what: "a truncated file", op: "page_count", bytes: fixture("truncated.pdf") },
    {
      what: "a truncated file through qpdf",
      op: "structure_check",
      bytes: fixture("truncated.pdf"),
    },
    { what: "bytes that are not a PDF", op: "page_count", bytes: fixture("not-a-pdf.bin") },
    { what: "a file with no pages", op: "page_count", bytes: fixture("no-pages.pdf") },
    { what: "an encrypted file", op: "page_count", bytes: fixture("encrypted.pdf") },
    { what: "the declared-size bomb", op: "page_count", bytes: fixture("xref-bomb.pdf") },
    // A file that parses PERFECTLY. qpdf's default logger fires for successful parses too,
    // which is what makes this case load-bearing rather than a control.
    { what: "a file that parses cleanly", op: "structure_check", bytes: fixture("pages-10.pdf") },
    // A recycle: a ceiling low enough that the lifecycle verdict comes back `true`.
    {
      what: "an operation that triggers recycling",
      op: "page_count",
      bytes: fixture("pages-10.pdf"),
      limits: { maxMemoryBytes: 1024 },
    },
  ];

  for (const testCase of cases) {
    await page.evaluate(
      ([op, bytes, attemptRecovery, limits]) =>
        window.burrowHarness.run(op as "page_count" | "structure_check", bytes as number[], {
          attemptRecovery: attemptRecovery as boolean,
          limits: limits as Record<string, number>,
        }),
      [
        testCase.op,
        testCase.bytes,
        testCase.attemptRecovery ?? false,
        testCase.limits ?? {},
      ] as const,
    );
  }

  // A WATCHDOG KILL, which is a failure path the list above cannot reach: the worker is
  // terminated mid-call, and a terminated worker is exactly where a half-written engine
  // message would surface.
  await page.evaluate(() => {
    window.burrowHarness.arm({ captureConsole: true, hangMs: 20_000 });
    window.burrowHarness.discardWorker();
  });
  await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes, { limits: { maxDurationMs: 1_000 } }),
    canaryBytes,
  );

  // A DELIBERATE ENGINE FAILURE, the ADR 0009 path. The exception's text must never be echoed.
  await page.evaluate(() => {
    window.burrowHarness.arm({ captureConsole: true, poison: "__burrow_pdfium_copy_in" });
    window.burrowHarness.discardWorker();
  });
  await page.evaluate((bytes) => window.burrowHarness.run("page_count", bytes), canaryBytes);

  const fromWorkers = await page.evaluate(() => window.burrowHarness.workerConsole());
  const everything = [...noise, ...fromWorkers.map((line) => `worker ${line}`)];

  const policyReports = everything.filter(isBrowserPolicyReport);
  const ours = everything.filter((line) => !isBrowserPolicyReport(line));

  expect(
    ours,
    `every byte here is suspect — offsets and object numbers are file content even when no ` +
      `canary is visible:\n${ours.join("\n")}`,
  ).toEqual([]);

  // THE EXCLUDED LINES ARE CHECKED TOO, rather than trusted. An exclusion that could swallow
  // file content would be worse than no assertion at all, so the canary is searched for in
  // exactly the lines the filter removed.
  expect(
    policyReports.join("\n"),
    "a browser policy report must never carry anything file-derived",
  ).not.toContain(mark);

  // Belt and braces, and cheap: if the assertion above is ever weakened, this still catches
  // the specific thing the fixture was built to provoke.
  expect(everything.join("\n")).not.toContain(mark);

  testInfo.annotations.push({
    type: "browser-policy-reports",
    description: `${policyReports.length} CSP report(s) from the browser itself`,
  });
});

test("the control: a worker that logs deliberately IS seen", async ({ page }) => {
  // Without this, a capture broken by a refactor would leave every case above green — and the
  // suite would report "nothing leaked" while measuring nothing at all. The same reason
  // `secret_leak.rs` runs `control-print-canary` before it believes any real case.
  const { mark } = loadCanary();

  // Same reason as above: let the first worker finish before replacing it.
  await openHarness(page);
  await page.evaluate((logCanary) => {
    window.burrowHarness.arm({ captureConsole: true, logCanary });
    window.burrowHarness.discardWorker();
  }, mark);
  await page.evaluate(() => window.burrowHarness.ready());

  const seen = await page.evaluate(() => window.burrowHarness.workerConsole());
  expect(
    seen.join("\n"),
    "the control logged the canary and the capture did not see it, so every assertion in " +
      "this file is measuring nothing",
  ).toContain(mark);
});
