// Two layers stop an unpoliced worker, and this checks the outer one.
//
// A dedicated worker created from a same-origin *script URL* does not inherit the creating
// document's CSP — it takes its policy from that script's HTTP response headers, and a static
// host sends none. Measured during 4a-i: such a worker ran with no policy at all and a
// cross-origin fetch from inside it reached the network, while every page-level CSP test
// passed.
//
//   * **Outer layer, tested here:** `worker-src blob:` — with no `'self'` — so the browser
//     refuses to construct a worker from a URL at all.
//   * **Inner layer:** the worker itself refuses to touch a file unless a policy is actually
//     in force, which it establishes by making a request the policy must refuse. That is a
//     property of the bundle rather than of the browser, so it is unit-tested in
//     `src/worker/guard.test.ts` where the environment can be controlled.
//
// `'self'` used to be in `worker-src` purely so this file could construct the bad worker and
// watch the inner layer refuse it. That was the test dictating the policy: it kept alive
// exactly the capability the design exists to remove. Both layers are stronger than either.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";

test("the browser refuses to build a worker from a URL", async ({ page }) => {
  await openHarness(page);

  const outcome = await page.evaluate(async () => {
    const violations: string[] = [];
    const note = (event: SecurityPolicyViolationEvent) => {
      violations.push(event.effectiveDirective || event.violatedDirective);
    };
    document.addEventListener("securitypolicyviolation", note);
    try {
      // Any same-origin script URL. It does not need to exist: `worker-src` is consulted
      // before the fetch, so a policy that permitted it would get a 404 rather than a
      // violation — which is exactly the difference being measured.
      new Worker("/engines/does-not-matter.js");
    } catch {
      // Chromium reports this asynchronously rather than throwing; Firefox and WebKit
      // differ. Either way the violation event is the signal.
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
    document.removeEventListener("securitypolicyviolation", note);
    return violations;
  });

  expect(
    outcome.some((directive) => directive.includes("worker-src")),
    `expected a worker-src violation, got ${JSON.stringify(outcome)}`,
  ).toBe(true);
});

test("a blob: worker is still permitted, so the policy is not simply forbidding workers", async ({
  page,
}) => {
  // The control. Without it, `worker-src 'none'` would pass the test above and break the
  // entire application — a policy that forbids everything is not the same as one that
  // forbids the right thing.
  await openHarness(page);

  const ran = await page.evaluate(async () => {
    const blob = new Blob(["self.postMessage('alive')"], { type: "text/javascript" });
    const worker = new Worker(URL.createObjectURL(blob));
    return new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => {
        worker.terminate();
        resolve(false);
      }, 5_000);
      worker.onmessage = () => {
        clearTimeout(timer);
        worker.terminate();
        resolve(true);
      };
      worker.onerror = () => {
        clearTimeout(timer);
        worker.terminate();
        resolve(false);
      };
    });
  });

  expect(ran, "a blob: worker must still be allowed to run").toBe(true);
});

test("the real worker is built from a blob and does the work", async ({ page }) => {
  // The end-to-end complement: the engines initialise, which they cannot do unless the
  // worker was constructed the permitted way and its own policy check passed.
  await openHarness(page);
  const ready = await page.evaluate(() => window.burrowHarness.workerInheritsCsp());
  expect(ready).toBe(true);
});
