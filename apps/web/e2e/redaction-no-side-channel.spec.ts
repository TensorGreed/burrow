// R9's named check (ADR 0006, #137): nothing leaves the heap before verification passes -- no
// OPFS or File System Access write, and no `blob:` URL handed to the page. `worker.terminate()`
// reclaims linear memory; it does not reclaim a file handle the page already holds.
//
// HELD MORE STRICTLY THAN THE ADR WORDS IT: no exit at all, from arming until the worker has gone
// quiet, on a success and on a refusal -- see `r9Violations`. The exits are stubbed in the
// worker's own scope before the bundle runs: ADR 0006's three and every other one found so far by
// trying to get bytes to the page past them. Each stub installs only where its exit exists, and
// the specs require the stubs armed to be EXACTLY the exits a worker in that browser can reach.
// SHOWN TO FAIL once per reachable exit, and once by an exit a timer defers past a refusal, each
// mutation asserted to have applied.
//
// WHAT IT DOES NOT SEE: an exit nobody has listed (the list is enumerated by hand, not complete),
// and one deferred by more than `SETTLE_MS`.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import {
  EXITS,
  PLANTS,
  THE_CALL,
  armedStubs,
  reachableExits,
  r9Violations,
  redactAndSettle,
} from "./redaction-protocol";

for (const [outcome, pageNumber] of [
  ["a successful redaction", 1],
  ["a refused redaction", 2],
] as const) {
  test(`${outcome} takes no exit at all`, async ({ page }) => {
    await openHarness(page);
    await page.evaluate(() => window.burrowHarness.armRedaction({ stubSideChannels: true }));
    const { reply, log } = await redactAndSettle(page, pageNumber);
    expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(pageNumber === 1);
    expect(armedStubs(log).length, "no stub installed").toBeGreaterThan(0);
    expect(r9Violations(log)).toEqual([]);
  });
}

test("the stubs armed are exactly the exits a worker here can reach", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({ stubSideChannels: true }));
  const { log } = await redactAndSettle(page, 1);
  const reachable = await reachableExits(page);
  // NOT EMPTY: a probe that reported nothing reachable would make the equality vacuous.
  expect(reachable, "the probe reported no reachable exit").toContain("createObjectURL");
  expect([...armedStubs(log)].sort()).toEqual([...reachable].sort());
  test.info().annotations.push(
    { type: "stubs armed", description: armedStubs(log).join(", ") },
    {
      type: "not reachable from a worker here",
      description:
        Object.keys(EXITS)
          .filter((name) => !reachable.includes(name))
          .join(", ") || "none",
    },
  );

  // A SharedArrayBuffer would be one more exit, and one no stub can watch: memory the page and the
  // worker both hold. It needs cross-origin isolation, so the page must not have it.
  expect(
    await page.evaluate(() => self.crossOriginIsolated),
    "the page is cross-origin isolated",
  ).toBe(false);
});

test("every exit a stub exists for has a planted copy, or no worker here can reach it", async ({
  page,
}) => {
  await openHarness(page);
  const reachable = await reachableExits(page);
  const unplanted = reachable.filter((name) => !(name in PLANTS));
  expect(
    unplanted,
    "a reachable exit with no planted copy is watched by a stub nobody has seen fire",
  ).toEqual([]);
});

for (const [exit, plant] of Object.entries(PLANTS)) {
  test(`SHOWN TO FAIL: a copy of the worker that takes ${exit} before the call is caught`, async ({
    page,
  }) => {
    await openHarness(page);
    if (!(await reachableExits(page)).includes(exit)) {
      test.skip(true, `${exit} is not reachable from a worker here, so there is nothing to take`);
    }
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ stubSideChannels: true, mutate }),
      { from: THE_CALL, to: `  ${plant}\n${THE_CALL}` },
    );
    expect(applied, `the planted ${exit} did not apply to the worker's copy`).toBe(true);

    const { log } = await redactAndSettle(page, 1);
    expect(r9Violations(log).join("\n")).toContain(`${exit} was called`);
  });
}

test("SHOWN TO FAIL: a copy that defers a blob: URL past a REFUSAL is caught", async ({ page }) => {
  await openHarness(page);
  const { applied } = await page.evaluate(
    (mutate) => window.burrowHarness.armRedaction({ stubSideChannels: true, mutate }),
    {
      from: THE_CALL,
      to: `  setTimeout(() => URL.createObjectURL(new Blob([bytes])), 0);\n${THE_CALL}`,
    },
  );
  expect(applied, "the deferred blob: URL did not apply to the worker's copy").toBe(true);

  const { reply, log } = await redactAndSettle(page, 2);
  expect(reply.ok, "the redaction was meant to be refused").toBe(false);
  expect(r9Violations(log).join("\n")).toContain("createObjectURL was called");
});
