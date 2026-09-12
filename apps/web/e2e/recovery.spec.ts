// ADR 0009's web contract, in a real browser, with real WebAssembly.
//
// > A test must assert this: panic deliberately, assert the instance is discarded, assert the
// > next operation succeeds on a fresh worker. Required by ADR 0006, not optional.
//
// `src/host/worker-host.test.ts` covers every state transition against a fake worker, in
// milliseconds, including the ones a browser will not reproduce on demand. What it cannot
// cover is whether the failure it models is the failure that actually happens — whether an
// exception out of the wasm module really does surface as a catchable one, whether the reply
// really carries `fatal`, and whether a terminated worker really can be replaced with a
// working one. That is this file.
//
// HOW THE FAILURE IS PRODUCED, AND WHY IT IS THE REAL ONE
//
// `harness.arm({ poison })` replaces one `__burrow_*` bridge global with a function that
// throws — in a test-only prologue prepended to the already-integrity-checked worker source,
// never by a hook in shipped code. A perfectly ordinary `page_count` then runs through the
// real Rust, which calls the bridge, and a real JavaScript exception propagates back out
// through the wasm frames.
//
// That is not a stand-in for ADR 0009's scenario. It is the scenario ADR 0009 calls the sneaky
// one, verbatim: "an Emscripten `abort()` inside an engine module throws a JS exception, which
// becomes an ordinary `Error::Internal` reply ... and leaves the worker alive with its init
// flags set, so nothing re-initialises."

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

function fixture(name: string): number[] {
  return Array.from(readFileSync(join(conformance, "fixtures", name)));
}

/** Arm the next worker, then discard the current one so the arming takes effect. */
async function armFreshWorker(
  page: import("@playwright/test").Page,
  arming: Parameters<Window["burrowHarness"]["arm"]>[0],
) {
  await page.evaluate((options) => {
    window.burrowHarness.arm(options);
    window.burrowHarness.discardWorker();
  }, arming);
}

test("a deliberate engine failure discards the worker, and the next operation succeeds", async ({
  page,
}) => {
  await openHarness(page);
  const before = await page.evaluate(() => window.burrowHarness.spawnCount());

  await armFreshWorker(page, { poison: "__burrow_pdfium_copy_in" });

  const poisoned = await page.evaluate(() =>
    window.burrowHarness.run("page_count", [0x25, 0x50, 0x44, 0x46]),
  );

  // The reply is `Internal` and `fatal`, and `fatal` was computed in Rust — the page reads it
  // and never re-derives it from `kind`.
  expect(poisoned.ok).toBe(false);
  expect(poisoned.kind).toBe("Internal");
  expect(poisoned.fatal).toBe(true);
  // NEVER THE TRAP TEXT. It is panic output and can carry input-derived bytes.
  expect(poisoned.message).not.toContain("deliberate engine failure");

  expect(await page.evaluate(() => window.burrowHarness.hasWorker())).toBe(false);
  expect(await page.evaluate(() => window.burrowHarness.state())).toBe("dead");

  // Disarm, then the required assertion: the next operation succeeds on a fresh worker.
  await page.evaluate(() => window.burrowHarness.arm({}));
  const recovered = await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes),
    fixture("pages-10.pdf"),
  );
  // The KIND, never the message. `Error::to_string()` renders qpdf's object numbers and byte
  // offsets, and a Playwright failure message is a CI log like any other.
  expect(recovered.ok, `unexpected ${recovered.kind}`).toBe(true);
  expect(recovered.pages).toBe(10);

  const after = await page.evaluate(() => window.burrowHarness.spawnCount());
  // Two more: the one armed to fail, and its replacement. Not three — nothing was retried.
  expect(after - before).toBe(2);
});

test("no ordinary outcome costs a worker, however hostile the file", async ({ page }) => {
  await openHarness(page);
  const before = await page.evaluate(() => window.burrowHarness.spawnCount());

  // Every failure a real file can produce, through both engines. One malformed PDF tearing
  // down the engine would let a single bad file poison a whole session.
  const cases: [string, string, "page_count" | "structure_check"][] = [
    ["truncated.pdf", "Malformed", "page_count"],
    ["not-a-pdf.bin", "Malformed", "page_count"],
    ["no-pages.pdf", "Malformed", "page_count"],
    ["encrypted.pdf", "PasswordRequired", "page_count"],
    ["xref-bomb.pdf", "LimitExceeded", "page_count"],
    ["truncated.pdf", "Malformed", "structure_check"],
  ];

  for (const [name, kind, op] of cases) {
    const reply = await page.evaluate(
      ([operation, bytes]) =>
        window.burrowHarness.run(operation as "page_count" | "structure_check", bytes as number[]),
      [op, fixture(name)] as const,
    );
    // `attemptRecovery` is off, so a truncated file is Malformed through qpdf too.
    expect(reply.ok, `${name} via ${op}`).toBe(false);
    expect(reply.kind, `${name} via ${op}`).toBe(kind);
    expect(reply.fatal, `${name} via ${op} must not poison the instance`).toBe(false);
  }

  expect(
    await page.evaluate(() => window.burrowHarness.spawnCount()),
    "six hostile files, zero workers spent",
  ).toBe(before);
  expect(await page.evaluate(() => window.burrowHarness.hasWorker())).toBe(true);
});

test("the watchdog kills a worker stuck inside a single engine call", async ({ page }) => {
  await openHarness(page);

  // ADR 0007 is explicit that checkpoint-based enforcement cannot interrupt one long engine
  // call: "A hostile file that makes a *single* PDFium call run for a minute is not stopped by
  // this, and we will not pretend otherwise." The prologue blocks the worker thread
  // SYNCHRONOUSLY inside `__burrow_pdfium_copy_in`, which is exactly that shape — an `await`
  // would leave the worker responsive and prove nothing.
  await armFreshWorker(page, { hangMs: 20_000 });

  const started = Date.now();
  const reply = await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes, { limits: { maxDurationMs: 1_500 } }),
    fixture("pages-10.pdf"),
  );
  const elapsed = Date.now() - started;

  expect(reply.ok).toBe(false);
  expect(reply.kind).toBe("LimitExceeded");
  expect(reply.limit).toBe("max_duration_ms");
  expect(reply.allowed).toBe("1500");
  // Killed near its deadline, not after the 20 s hang would have finished. Generous, because
  // a CI runner is not a benchmark; the claim is "interrupted", not "interrupted promptly".
  expect(elapsed, `took ${elapsed}ms`).toBeLessThan(10_000);

  expect(await page.evaluate(() => window.burrowHarness.state())).toBe("dead");

  // And the page is usable afterwards. A watchdog that leaves the page broken would trade one
  // failure for a worse one.
  await page.evaluate(() => window.burrowHarness.arm({}));
  const recovered = await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes),
    fixture("pages-10.pdf"),
  );
  expect(recovered.ok, `unexpected ${recovered.kind}`).toBe(true);
});

test("the circuit breaker stops a respawn loop, and only reset() restarts it", async ({ page }) => {
  await openHarness(page);

  // A file that crashes every worker it touches. Without a breaker this is an unbounded
  // respawn loop: each crash costs a 6.5 MB engine compile, and the tab does nothing else.
  //
  // THREE crashes, not four, and each one asserted to be a CRASH rather than merely fatal.
  // A fourth iteration would already be past the breaker and would return
  // `EngineUnavailable` — which an `expect(reply.fatal)` could not tell apart from a crash,
  // so the loop would pass while testing one fewer crash than it claimed.
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await armFreshWorker(page, { poison: "__burrow_pdfium_copy_in" });
    const reply = await page.evaluate(() =>
      window.burrowHarness.run("page_count", [0x25, 0x50, 0x44, 0x46]),
    );
    expect(reply.kind, `attempt ${attempt}`).toBe("Internal");
    expect(reply.fatal, `attempt ${attempt}`).toBe(true);
  }

  const spawns = await page.evaluate(() => window.burrowHarness.spawnCount());
  const refused = await page.evaluate(() =>
    window.burrowHarness.run("page_count", [0x25, 0x50, 0x44, 0x46]),
  );

  expect(refused.kind).toBe("EngineUnavailable");
  expect(await page.evaluate(() => window.burrowHarness.breakerOpen())).toBe(true);
  expect(
    await page.evaluate(() => window.burrowHarness.spawnCount()),
    "an open breaker must spawn nothing at all",
  ).toBe(spawns);

  // Only a deliberate gesture reopens it — a "try again" button, not the next file. A page
  // that retried on a timer would resume the loop at a slower rate rather than end it.
  await page.evaluate(() => {
    window.burrowHarness.arm({});
    window.burrowHarness.reset();
  });
  const recovered = await page.evaluate(
    (bytes) => window.burrowHarness.run("page_count", bytes),
    fixture("pages-10.pdf"),
  );
  expect(recovered.ok, `unexpected ${recovered.kind}`).toBe(true);
  expect(await page.evaluate(() => window.burrowHarness.spawnCount())).toBe(spawns + 1);
});

test("the SAME file handle survives the worker that was reading it", async ({ page }) => {
  // THE REASON THE INPUT IS A BLOB, and the test has to be shaped carefully to say so.
  //
  // A transferred `ArrayBuffer` is detached page-side the moment it is posted, so after a
  // worker dies the caller holds nothing and cannot retry. A Blob is passed by reference: the
  // page never materialises the bytes, and the handle outlives the worker reading it.
  //
  // Sending fresh bytes twice would NOT test that. A transfer-based implementation would pass
  // such a test identically, because nothing would be reused and so nothing could be detached.
  // So one `File` is created in page scope and every operation below runs against that object.
  await openHarness(page);
  await page.evaluate((b) => window.burrowHarness.holdFile(b), fixture("pages-10.pdf"));

  await armFreshWorker(page, { poison: "__burrow_pdfium_copy_in" });
  const failed = await page.evaluate(() => window.burrowHarness.runHeld("page_count"));
  expect(failed.fatal).toBe(true);

  // Still readable. A detached buffer would be zero-length here, which is the difference the
  // whole decision turns on.
  expect(
    await page.evaluate(() => window.burrowHarness.heldIsStillReadable()),
    "the handle must survive the worker that was reading it",
  ).toBe(true);

  await page.evaluate(() => window.burrowHarness.arm({}));
  const retried = await page.evaluate(() => window.burrowHarness.runHeld("page_count"));
  expect(retried.ok, `unexpected ${retried.kind}`).toBe(true);
  expect(retried.pages).toBe(10);
});
