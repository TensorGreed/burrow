// The engines load in a real browser, under the real CSP, and answer correctly.
//
// This is the half `cargo test` cannot reach. The orchestration above the bridge is covered
// on every target in `core/burrow-engines/src/web/tests.rs` against a fake bridge; what
// needs a browser is the loading path — the integrity-pinned fetch, the streaming compile,
// `instantiateWasm`, the classic worker, and whether two Emscripten modules coexist.
//
// The expectations come from `tests/conformance/expectations.json`, the same file the
// native conformance test reads. Neither side restates them. PR 4b turns that into a full
// differential harness across three browsers; this is the first use of it from the web.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

interface Case {
  name: string;
  file: string;
  password: string | null;
  expect: { ok?: { page_count: number }; err?: string };
}

const expectations = JSON.parse(readFileSync(join(conformance, "expectations.json"), "utf8")) as {
  schema: number;
  cases: Case[];
};

// A newer schema means the shape changed. Fail rather than guess — the same rule the native
// reader follows, and for the same reason: a silently-misread expectation is worse than no
// expectation.
if (expectations.schema !== 1) {
  throw new Error(`unsupported expectations schema ${expectations.schema}`);
}

function fixtureBytes(relative: string): number[] {
  return Array.from(readFileSync(join(conformance, relative)));
}

interface Reply {
  ok: boolean;
  kind: string;
  fatal: boolean;
  message: string;
  pages: number;
  limit: string;
  requested: number;
  allowed: number;
}

async function openHarness(page: Page) {
  await page.goto("/harness");
  await expect(page.locator("#status")).toHaveText("engines ready", { timeout: 60_000 });
}

async function run(
  page: Page,
  op: "page_count" | "structure_check",
  bytes: number[],
  options: { password?: number[] } = {},
): Promise<Reply> {
  return page.evaluate(
    ([op, bytes, options]) =>
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (window as any).burrowHarness.run(op, bytes, options) as Promise<Reply>,
    [op, bytes, options] as const,
  );
}

test("the engines initialise inside a worker under the generated CSP", async ({ page }) => {
  await openHarness(page);
});

test("every conformance case produces the outcome the corpus records", async ({ page }) => {
  await openHarness(page);

  for (const testCase of expectations.cases) {
    const password = testCase.password
      ? Array.from(new TextEncoder().encode(testCase.password))
      : undefined;
    const reply = await run(page, "page_count", fixtureBytes(testCase.file), { password });

    if (testCase.expect.ok) {
      expect(reply.ok, `${testCase.name}: ${reply.kind} ${reply.message}`).toBe(true);
      expect(reply.pages, `${testCase.name}: page count`).toBe(testCase.expect.ok.page_count);
    } else {
      expect(reply.ok, `${testCase.name}: expected a failure`).toBe(false);
      expect(reply.kind, `${testCase.name}: error variant`).toBe(testCase.expect.err);
    }

    // Whatever the outcome, none of these is an engine we can no longer reason about. A
    // `fatal` here would mean a corpus file bricks a worker, which is a real bug.
    expect(reply.fatal, `${testCase.name}: must not poison the instance`).toBe(false);
  }
});

test("qpdf answers through the same worker, with its logging silenced", async ({ page }) => {
  const console_messages: string[] = [];
  page.on("console", (message) => console_messages.push(message.text()));

  await openHarness(page);
  const reply = await run(page, "structure_check", fixtureBytes("fixtures/pages-10.pdf"));

  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);
  expect(reply.pages).toBe(10);

  // A first indication only. The thorough version — every failure path, canary fixtures,
  // worker consoles as well as the page's, and a control case that deliberately logs — is
  // 4a-ii's console-silence test. Asserting the weaker thing here would be worth little
  // on its own; it is here because a regression would show up immediately.
  expect(console_messages.join("\n")).not.toMatch(/WARNING|offset|object \d/i);
});

test("the declared-size bomb is refused before the engine allocates", async ({ page }) => {
  await openHarness(page);
  const reply = await run(page, "page_count", fixtureBytes("fixtures/xref-bomb.pdf"));

  expect(reply.ok).toBe(false);
  expect(reply.kind).toBe("LimitExceeded");
  expect(reply.limit).toBe("max_memory_bytes");
  // The pre-scan is pure Rust and runs before anything crosses the bridge, so this must not
  // be a worker-killing failure: refusing a hostile file is a normal outcome.
  expect(reply.fatal).toBe(false);
});

test("an encrypted document reports PasswordRequired rather than a page count", async ({
  page,
}) => {
  await openHarness(page);
  const reply = await run(page, "page_count", fixtureBytes("fixtures/encrypted.pdf"));

  expect(reply.ok).toBe(false);
  expect(reply.kind).toBe("PasswordRequired");
  expect(reply.fatal).toBe(false);
});
