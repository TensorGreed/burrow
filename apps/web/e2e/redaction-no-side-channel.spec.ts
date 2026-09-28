// R9's named check (ADR 0006, #137): nothing leaves the heap before verification passes -- no
// OPFS or File System Access write, and no `blob:` URL handed to the page. `worker.terminate()`
// reclaims linear memory; it does not reclaim a file handle the page already holds.
//
// The three exits ADR 0006 names are stubbed in the worker's own scope before the bundle runs, and
// three more besides -- BroadcastChannel, IndexedDB and the Cache API, each also a way out of the
// heap the page can read back. Each stub reports its call in order with everything else the worker
// posts, and they report which of them installed, so "never called" is distinguishable from "never
// watched". SHOWN TO FAIL twice, by copies of the worker that make a `blob:` URL of the input and
// that broadcast it before the call, each mutation asserted to have applied.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import {
  THE_CALL,
  armedStubs,
  onlyRequest,
  r9Violations,
  redactWriter,
} from "./redaction-protocol";

test("no exit is taken before the verified reply", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({ stubSideChannels: true }));
  const reply = await redactWriter(page, 1);
  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);

  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  const armed = armedStubs(log);
  // THE TWO THAT EVERY WORKER SCOPE CAN BE STUBBED FOR, required. `getDirectory` is required where
  // the worker has a `StorageManager` at all, and its absence is reported rather than passed over.
  expect(armed, "the blob: and save-dialog stubs did not install").toEqual(
    expect.arrayContaining(["createObjectURL", "showSaveFilePicker"]),
  );
  test.info().annotations.push({ type: "stubs armed", description: armed.join(", ") });
  expect(r9Violations(log, onlyRequest(log))).toEqual([]);
});

/**
 * The exits a worker CAN reach here, each named by the interface its stub wraps. Asked of a
 * throwaway worker rather than the page, because the two scopes differ -- and the exit is the
 * worker's.
 */
const REACHABLE = `self.postMessage({
  getDirectory: typeof StorageManager !== "undefined",
  BroadcastChannel: typeof BroadcastChannel !== "undefined",
  indexedDB: typeof IDBFactory !== "undefined",
  caches: typeof CacheStorage !== "undefined",
})`;

test("every exit the worker can reach is watched", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({ stubSideChannels: true }));
  await redactWriter(page, 1);
  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  const reachable = await page.evaluate(async (source) => {
    const probe = new Worker(URL.createObjectURL(new Blob([source], { type: "text/javascript" })));
    return new Promise<Record<string, boolean>>((done) => {
      probe.onmessage = (event) => {
        done(event.data);
        probe.terminate();
      };
    });
  }, REACHABLE);
  // A SharedArrayBuffer would be a seventh exit, and one no stub can watch: memory the page and
  // the worker both hold. It needs cross-origin isolation, so the page must not have it.
  expect(
    await page.evaluate(() => self.crossOriginIsolated),
    "the page is cross-origin isolated",
  ).toBe(false);
  const expected = Object.entries(reachable)
    .filter(([, here]) => here)
    .map(([name]) => name);
  // NOT EMPTY: a probe that reported nothing reachable would make this assertion vacuous.
  expect(
    expected.length,
    `the probe reported no reachable exit: ${JSON.stringify(reachable)}`,
  ).toBeGreaterThan(0);
  expect(armedStubs(log).sort()).toEqual(expect.arrayContaining(expected.sort()));
  test.info().annotations.push({
    type: "not reachable from a worker here",
    description:
      Object.keys(reachable)
        .filter((name) => !reachable[name])
        .join(", ") || "none",
  });
});

test("SHOWN TO FAIL: a copy of the worker that makes a blob: URL before the call is caught", async ({
  page,
}) => {
  await openHarness(page);
  const { applied } = await page.evaluate(
    (mutate) => window.burrowHarness.armRedaction({ stubSideChannels: true, mutate }),
    { from: THE_CALL, to: `  URL.createObjectURL(new Blob([bytes]));\n${THE_CALL}` },
  );
  expect(applied, "the planted blob: URL did not apply to the worker's copy").toBe(true);

  await redactWriter(page, 1);
  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  expect(r9Violations(log, onlyRequest(log)).join("\n")).toMatch(
    /createObjectURL was called .* before the verified reply/,
  );
});

test("SHOWN TO FAIL: a copy of the worker that broadcasts the input before the call is caught", async ({
  page,
}) => {
  await openHarness(page);
  const { applied } = await page.evaluate(
    (mutate) => window.burrowHarness.armRedaction({ stubSideChannels: true, mutate }),
    { from: THE_CALL, to: `  new BroadcastChannel("out").postMessage(bytes);\n${THE_CALL}` },
  );
  expect(applied, "the planted broadcast did not apply to the worker's copy").toBe(true);

  await redactWriter(page, 1);
  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  expect(r9Violations(log, onlyRequest(log)).join("\n")).toMatch(
    /BroadcastChannel was called .* before the verified reply/,
  );
});
