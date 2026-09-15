// The engines load in a real browser, under the real CSP, and answer at all.
//
// This is the half `cargo test` cannot reach. The orchestration above the bridge is covered on
// every target in `core/burrow-engines/src/web/tests.rs` against a fake bridge; what needs a
// browser is the loading path — the integrity-pinned fetch, the streaming compile,
// `instantiateWasm` and the classic worker. It also covered whether two Emscripten modules
// coexist, until spike 0004 took PDFium out of the payload and left that property without a
// subject; the last test in this file records what happened to it.
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

test("qpdf answers a page count through the worker, and the number is real", async ({ page }) => {
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
  // its own; it is here because a regression would show up immediately on the operation whose
  // engine is the one that logs.
  expect(console_messages.join("\n")).not.toMatch(/WARNING|offset|object \d/i);
});

test("the two operations reach the same engine, so their page counts cannot disagree", async ({
  page,
}) => {
  // THIS TEST USED TO BE "both engines are live in one worker and answer independently", and
  // it was still passing after spike 0004 took PDFium out of the payload — over a premise
  // that no longer had a subject. Both calls below went to qpdf, so "two Emscripten modules
  // coexist" was being confirmed by one module answering twice. That is the failure the root
  // CLAUDE.md names: a check that silently examines nothing reads as coverage.
  //
  // What replaced it is the property the substitution actually created. `page_count` was
  // PDFium and `structure_check` was qpdf, so the count a person saw on choosing a file and
  // the count the write path worked from came from two engines that could differ — which is
  // what #61 was about. They are one engine now, at one posture, so they must agree, and a
  // future change that repoints either of them fails here.
  //
  // The coexistence property is not deleted because it stopped mattering; it is deleted
  // because it has no subject. If M2 puts a second module back, this is where it returns.
  await openHarness(page);
  const bytes = fixtureBytes("fixtures/pages-10.pdf");
  const viaPageCount = await run(page, "page_count", bytes);
  const viaStructureCheck = await run(page, "structure_check", bytes);

  expect(viaPageCount.ok && viaStructureCheck.ok, "both operations must answer").toBe(true);
  expect(
    viaPageCount.pages,
    "page_count and structure_check are the same engine at the same posture; a disagreement " +
      "means one of them was repointed",
  ).toBe(viaStructureCheck.pages);
  expect(viaPageCount.pages).toBe(10);
});
