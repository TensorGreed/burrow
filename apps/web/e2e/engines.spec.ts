// The engines load in a real browser, under the real CSP, and answer at all.
//
// This is the half `cargo test` cannot reach. The orchestration above the bridge is covered on
// every target in `core/burrow-engines/src/web/tests.rs` against a fake bridge; what needs a
// browser is the loading path — the integrity-pinned fetch, the streaming compile,
// `instantiateWasm`, the classic worker, and whether two Emscripten modules coexist.
//
// WHAT MOVED OUT OF THIS FILE IN PR 4b, AND WHY
//
// It used to read `tests/conformance/expectations.json` and loop over every case. That is now
// `e2e/conformance.spec.ts`, which does it properly: both engines rather than one, the case's
// own limits, the failure's *stage* as well as its kind, and a diff against what the native
// implementation produced for the same corpus.
//
// Leaving the loop here as well would have meant **two readers of one contract** in the same
// language, which is the drift `expectations.json` exists to prevent — and the weaker of the
// two would have been the one that quietly stopped covering things. So this file keeps only
// what is genuinely its own: that a browser can load the engines and get an answer back.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { openHarness, type Reply } from "./harness";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

function fixtureBytes(relative: string): number[] {
  return Array.from(readFileSync(join(conformance, relative)));
}

async function run(
  page: Page,
  op: "page_count" | "structure_check",
  bytes: number[],
): Promise<Reply> {
  return page.evaluate(([op, bytes]) => window.burrowHarness.run(op, bytes), [op, bytes] as const);
}

test("the engines initialise inside a worker under the generated CSP", async ({ page }) => {
  await openHarness(page);
});

test("PDFium answers through the worker with a real page count", async ({ page }) => {
  await openHarness(page);
  const reply = await run(page, "page_count", fixtureBytes("fixtures/pages-137.pdf"));

  // The KIND, never the message. `Error::to_string()` renders qpdf's object numbers and byte
  // offsets, and a Playwright failure message is a CI log like any other — `secret_leak.rs`'s
  // discipline does not stop at the process boundary.
  expect(reply.ok, `unexpected ${reply.kind}`).toBe(true);
  // An awkward number on purpose: an off-by-one or a truncated count is visible in a way it
  // would not be for 1, 10, or a power of two. Every *other* fixture's count is asserted by
  // the conformance harness.
  expect(reply.pages).toBe(137);
});

test("qpdf answers through the same worker, with its logging silenced", async ({ page }) => {
  const console_messages: string[] = [];
  page.on("console", (message) => console_messages.push(message.text()));

  await openHarness(page);
  const reply = await run(page, "structure_check", fixtureBytes("fixtures/pages-10.pdf"));

  expect(reply.ok, `unexpected ${reply.kind}`).toBe(true);
  expect(reply.pages).toBe(10);

  // A first indication only. The thorough version — every failure path, canary fixtures,
  // worker consoles as well as the page's, and a control case that deliberately logs — is
  // `e2e/console-silence.spec.ts`. Asserting the weaker thing here would be worth little on
  // its own; it is here because a regression would show up immediately, in the test that also
  // proves the two modules coexist.
  expect(console_messages.join("\n")).not.toMatch(/WARNING|offset|object \d/i);
});

test("both engines are live in one worker and answer independently", async ({ page }) => {
  // ADR 0006 requirement 1's practical consequence: `pdfium.js` is not modularised and its
  // state lives in worker globals, so "two Emscripten modules coexist" is a property to check
  // rather than assume. The conformance harness leans on it for every case; this is the test
  // that says so directly.
  await openHarness(page);
  const bytes = fixtureBytes("fixtures/pages-10.pdf");
  const viaPdfium = await run(page, "page_count", bytes);
  const viaQpdf = await run(page, "structure_check", bytes);

  expect(viaPdfium.ok && viaQpdf.ok, "both engines must answer in the same worker").toBe(true);
  expect(viaPdfium.pages).toBe(10);
  expect(viaQpdf.pages).toBe(10);
});
