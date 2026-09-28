// R9's named check (ADR 0006, #137): nothing leaves the heap before verification passes -- no
// OPFS or File System Access write, and no `blob:` URL handed to the page. `worker.terminate()`
// reclaims linear memory; it does not reclaim a file handle the page already holds.
//
// HELD MORE STRICTLY THAN THE ADR WORDS IT: no exit at all, from arming until the worker has gone
// quiet, on a success and on a refusal -- see `r9Violations`. Two halves:
// - STUBS in the worker's own scope, installed before the bundle runs: ADR 0006's three and every
//   other one found so far by trying to get bytes to the page past them. Each installs only where
//   its exit exists, and the stubs armed must be EXACTLY the exits a worker in that browser can
//   reach. They are a tripwire for a regression, not a boundary against a bundle set on evading
//   them: they share its realm.
// - THE TEST SERVER'S LOG, which no code in the browser can touch: every request in the window is
//   a GET from the site of one of its own four artifacts, by path, so a query string carrying
//   bytes is caught.
// SHOWN TO FAIL once per reachable exit, by the two evasions of the stubs measured so far, by an
// exit a timer defers past a refusal, and by a fetch that carries bytes -- each mutation asserted
// to have applied.
//
// WHAT IT DOES NOT SEE: an exit nobody has listed (the list is enumerated by hand, not complete),
// and one later than `2 * SETTLE_MS` after the settle handshake, on the page's clock.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import {
  EXITS,
  PLANTS,
  THE_CALL,
  armedStubs,
  networkViolations,
  onlyRequest,
  r8Violations,
  reachableExits,
  r9Violations,
  redactAndSettle,
} from "./redaction-protocol";
import { mark, since } from "./request-log";

for (const [outcome, pageNumber] of [
  ["a successful redaction", 1],
  ["a refused redaction", 2],
] as const) {
  test(`${outcome} takes no exit at all, and makes no request but its own artifacts`, async ({
    page,
  }, testInfo) => {
    await openHarness(page);
    await page.evaluate(() => window.burrowHarness.armRedaction({ stubSideChannels: true }));
    const marker = `r9-${testInfo.project.name}-${pageNumber}-${Date.now()}`;
    await mark(marker);
    const { reply, log, nonce } = await redactAndSettle(page, pageNumber);
    expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(pageNumber === 1);
    expect(armedStubs(log).length, "no stub installed").toBeGreaterThan(0);
    expect(r9Violations(log, nonce)).toEqual([]);
    // AND R8 OVER THE SAME LOG, where the armed list is real: the near-miss for its exemption.
    expect(r8Violations(log, onlyRequest(log), nonce)).toEqual([]);

    const requests = since(marker);
    // WHAT THE NETWORK HALF EXAMINED, by name: a fresh worker's own fetches.
    testInfo.annotations.push({
      type: "requests in the window",
      description: requests.map((entry) => entry.url).join(", ") || "none",
    });
    expect(networkViolations(requests, new URL(page.url()).origin)).toEqual([]);
    // AND EXACTLY THE FOUR, one each: a window that saw nothing would pass every rule above, and a
    // fresh worker is measured to fetch all four inside it in all three browsers.
    expect(
      requests.map((entry) => entry.url.replace(/\.[0-9a-f]{16}\./, ".<hash>.")).sort(),
      "the window did not hold redaction's four artifacts, once each",
    ).toEqual([
      "/engines/burrow-redact-worker.<hash>.js",
      "/engines/burrow_wasm_redact_bg.<hash>.wasm",
      "/engines/control.<hash>.txt",
      "/engines/qpdf.<hash>.wasm",
    ]);
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

/** Each planted exit, the exit its stub must report, and the page the redaction runs on. */
const TAKEN: { name: string; exit: string; plant: string; pageNumber: number }[] = [
  ...Object.entries(PLANTS).map(([exit, plant]) => ({
    name: `takes ${exit}`,
    exit,
    plant,
    pageNumber: 1,
  })),
  {
    // A bundle that filters its own reports out of `self.postMessage` first: the stubs post
    // through a reference captured before the bundle ran.
    name: "silences the stubs' reports, then makes a blob: URL",
    exit: "createObjectURL",
    plant:
      "{ const P = self.postMessage.bind(self); self.postMessage = (m, t) => (m && m.__burrowSideChannel ? undefined : P(m, t)); URL.createObjectURL(new Blob([bytes])); }",
    pageNumber: 1,
  },
  {
    // The Proxy forwards `.prototype` to its target, so the prototype's back-reference had to be
    // replaced too.
    name: "builds a nested worker through Worker.prototype.constructor",
    exit: "Worker",
    plant: 'new Worker.prototype.constructor(self.location.href, { name: "nested" });',
    pageNumber: 1,
  },
  {
    // A REFUSAL, where nothing was ever verified, and the exit deferred past the reply.
    name: "defers a blob: URL past a refusal",
    exit: "createObjectURL",
    plant: "setTimeout(() => URL.createObjectURL(new Blob([bytes])), 0);",
    pageNumber: 2,
  },
];

for (const taken of TAKEN) {
  test(`SHOWN TO FAIL: a copy of the worker that ${taken.name} is caught`, async ({ page }) => {
    await openHarness(page);
    if (!(await reachableExits(page)).includes(taken.exit)) {
      test.skip(
        true,
        `${taken.exit} is not reachable from a worker here, so there is nothing to take`,
      );
    }
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ stubSideChannels: true, mutate }),
      { from: THE_CALL, to: `  ${taken.plant}\n${THE_CALL}` },
    );
    expect(applied, `the planted exit did not apply to the worker's copy: ${taken.name}`).toBe(
      true,
    );

    const { reply, log, nonce } = await redactAndSettle(page, taken.pageNumber);
    expect(reply.ok, "the redaction did not end as the case meant").toBe(taken.pageNumber === 1);
    expect(r9Violations(log, nonce).join("\n")).toContain(`${taken.exit} was called`);
    // A STUB'S REPORT IS THE HARNESS'S OWN, and R8 must pass it: the near-miss for that exemption.
    expect(r8Violations(log, onlyRequest(log), nonce)).toEqual([]);
  });
}

test("SHOWN TO FAIL: a copy of the worker that fetches its own module with bytes in the query is caught", async ({
  page,
}, testInfo) => {
  await openHarness(page);
  const { applied } = await page.evaluate(
    (mutate) => window.burrowHarness.armRedaction({ stubSideChannels: true, mutate }),
    {
      from: THE_CALL,
      to: `  fetch(performance.getEntriesByType("resource").map((e) => e.name).find((n) => n.endsWith(".wasm")) + "?leak=" + Array.from(bytes.subarray(0, 8)).join("-")).catch(() => {});\n${THE_CALL}`,
    },
  );
  expect(applied, "the planted fetch did not apply to the worker's copy").toBe(true);
  const marker = `r9-fetch-${testInfo.project.name}-${Date.now()}`;
  await mark(marker);

  const { log, nonce } = await redactAndSettle(page, 1);
  // EVERY STUB STAYED SILENT, which is the point: only the server's log can see this one.
  expect(r9Violations(log, nonce)).toEqual([]);
  expect(networkViolations(since(marker), new URL(page.url()).origin).join("\n")).toMatch(
    /\?leak=37-80-68-70/,
  );
});

// THE SETTLE RULE, on logs written by hand: a log without this nonce's echo is refused, and the
// same log with it passes.
test("SHOWN TO FAIL, by hand: a log read before the worker went quiet is refused", () => {
  const echo = {
    sent: null,
    id: null,
    keys: ["__burrowSettled"],
    bytes: 0,
    fields: { __burrowSettled: 42 },
    shape: "{__burrowSettled:number}",
    ports: 0,
    ok: null,
    sideChannel: null,
    armed: null,
    settled: 42,
  };
  expect(r9Violations([], 42).join("\n")).toMatch(/before the worker went quiet/);
  expect(r9Violations([{ ...echo, settled: 41 }], 42).join("\n")).toMatch(
    /before the worker went quiet/,
  );
  expect(r9Violations([echo], 42)).toEqual([]);
});

// THE NETWORK RULES, on logs written by hand: each artifact once, by GET, from the site, passes;
// the same artifact twice, another method, and another origin are each refused.
test("SHOWN TO FAIL, by hand: a request that is not one GET of an artifact from the site is refused", () => {
  const site = "http://localhost:4321";
  const entry = { origin: site, method: "GET", at: 0, agent: "" };
  const once = [
    { ...entry, url: "/engines/qpdf.0123456789abcdef.wasm" },
    { ...entry, url: "/engines/control.0123456789abcdef.txt" },
  ];
  expect(networkViolations(once, site)).toEqual([]);
  expect(networkViolations([...once, once[0]], site).join("\n")).toMatch(/requested twice/);
  expect(networkViolations([{ ...once[0], method: "POST" }], site).join("\n")).toMatch(
    /is not a GET/,
  );
  expect(
    networkViolations([{ ...once[0], origin: "http://localhost:4322" }], site).join("\n"),
  ).toMatch(/is not a GET/);
});
