import { test, expect } from "@playwright/test";

// ADR 0006's bar, as executable assertions. Everything here runs in a real headless
// Chromium against a real Web Worker; nothing is simulated in Node.

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => console.log("[pageerror]", String(e).slice(0, 120)));
  await page.goto("/index.html");
  await page.waitForFunction(() => !!window.spike);
});

test("a real PDF opens in a worker, driven from Rust, and reports its page count", async ({ page }) => {
  const r = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "pdfium", name: "medium-100page.pdf" }),
  );
  expect(r.ok).toBe(true);
  expect(r.isOk).toBe(true);
  expect(r.pages).toBe(100); // the generator made exactly 100 pages
});

test("qpdf is linked and callable in the same worker", async ({ page }) => {
  const r = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "qpdf", name: "medium-100page.pdf" }),
  );
  expect(r.ok).toBe(true);
  expect(r.isOk).toBe(true);
  expect(r.pages).toBe(100);
});

test("malformed input through qpdf is a typed error, not an abort", async ({ page }) => {
  // The C++ exceptions test. A throw inside qpdf must become a typed error, and the
  // worker must survive it -- so a second call still works.
  const bad = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "qpdf", name: "malformed-truncated.pdf" }),
  );
  expect(bad.ok).toBe(true);          // the worker replied at all
  expect(bad.workerDied).toBeFalsy(); // it did not abort
  expect(bad.isOk).toBe(false);
  expect(bad.error).toBe(1);          // SpikeError::Malformed

  const notPdf = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "qpdf", name: "malformed-notpdf.bin" }),
  );
  expect(notPdf.workerDied).toBeFalsy();
  expect(notPdf.error).toBe(1);

  // Survival check: the same worker still answers correctly afterwards.
  const after = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "qpdf", name: "small-1page.pdf" }),
  );
  expect(after.isOk).toBe(true);
  expect(after.pages).toBe(1);
  expect(await page.evaluate(() => window.spike.deaths())).toBe(0);
});

test("malformed input through pdfium is a typed error, not an abort", async ({ page }) => {
  const r = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "pdfium", name: "malformed-truncated.pdf" }),
  );
  expect(r.workerDied).toBeFalsy();
  expect(r.isOk).toBe(false);
  expect(r.error).toBe(1);
});

test("a Rust panic traps as a catchable JS error; the instance must still be discarded", async ({ page }) => {
  // Measured behaviour, which is NOT what apps/web/CLAUDE.md currently claims.
  //
  // On wasm32-unknown-unknown a panic ends in the `unreachable` instruction. That is a
  // WebAssembly trap, and a trap surfaces in JS as a catchable RuntimeError -- it does
  // NOT kill the worker. Calls afterwards still appear to work.
  //
  // "Appear" is the operative word: the Rust module's invariants are broken after a
  // trap. It only looks fine here because this spike's Rust holds no state and, in
  // option 1, the engines live in *separate* modules whose heaps the trap never touched.
  // So the page must discard the instance deliberately; nothing forces its hand.
  const trapped = await page.evaluate(() => window.spike.call({ cmd: "panic" }, 20000));
  expect(trapped.ok).toBe(false);
  expect(trapped.threw).toContain("unreachable");
  expect(await page.evaluate(() => window.spike.deaths())).toBe(0); // worker survived

  // The engines are genuinely unharmed, because they are in other modules.
  const stillWorks = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "qpdf", name: "medium-100page.pdf" }),
  );
  expect(stillWorks.pages).toBe(100);

  // The respawn path must still work when the page invokes it, since that is what a
  // real app has to do after a trap rather than trusting the survivor.
  await page.evaluate(() => window.spike.discardWorker());
  expect(await page.evaluate(() => window.spike.deaths())).toBe(1);
  const revived = await page.evaluate(() =>
    window.spike.call({ cmd: "open", engine: "pdfium", name: "small-1page.pdf" }),
  );
  expect(revived.ok).toBe(true);
  expect(revived.pages).toBe(1);
});

test("measurements", async ({ page }) => {
  const cold = await page.evaluate(() => window.spike.coldLoad("small-1page.pdf"));
  const rows = [];
  for (const name of ["small-1page.pdf", "medium-100page.pdf", "large-50mb.pdf"]) {
    for (const engine of ["pdfium", "qpdf"]) {
      const r = await page.evaluate(
        ([e, n]) => window.spike.call({ cmd: "open", engine: e, name: n }),
        [engine, name],
      );
      rows.push({ engine, name, pages: r.pages, ms: r.ms, engineHeap: r.engineHeap });
    }
  }
  console.log("\nMEASUREMENTS_JSON " + JSON.stringify({ coldMs: cold.coldMs, rows }));
  expect(rows.every((r) => r.pages >= 1)).toBe(true);
});
